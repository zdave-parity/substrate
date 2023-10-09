// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use super::{super::block_provider::BlockProvider, core::Core, in_substreams::InSubstreams};
use futures::{FutureExt, StreamExt};
use libp2p::{
	core::upgrade::{write_length_prefixed, ReadyUpgrade},
	swarm::{
		handler::{
			ConnectionEvent, ConnectionHandler, ConnectionHandlerEvent, ConnectionHandlerUpgrErr,
			DialUpgradeError, FullyNegotiatedInbound, FullyNegotiatedOutbound, KeepAlive,
			ListenUpgradeError, SubstreamProtocol,
		},
		NegotiatedSubstream,
	},
	PeerId,
};
use log::debug;
use std::{
	future::Future,
	pin::Pin,
	sync::Arc,
	task::{Context, Poll},
	time::{Duration, Instant},
};

const LOG_TARGET: &str = "ipfs::bitswap";

// Currently only support this version of the protocol
const PROTOCOL_NAME: &[u8] = b"/ipfs/bitswap/1.2.0";

/// "Soft" maximum number of pending blocks/presences per connection. We will continue to read from
/// inbound substreams until the number of pending blocks/presences rises above this number. Note
/// that as we only provide back-pressure between inbound messages, it is possible for the number
/// of pending blocks/presences to rise significantly above this limit.
const SOFT_MAX_PENDING: usize = 1000;

/// Minimum time to keep connections alive after becoming idle.
const IDLE_KEEP_ALIVE: Duration = Duration::from_secs(5);

enum OutSubstream {
	None,
	Opening,
	Idle(NegotiatedSubstream),
	Writing(Pin<Box<dyn Future<Output = std::io::Result<NegotiatedSubstream>> + Send>>),
	UpgradeError(ConnectionHandlerUpgrErr<void::Void>),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Io(#[from] std::io::Error),
	#[error("Substream upgrade failed: {0}")]
	Upgrade(ConnectionHandlerUpgrErr<void::Void>),
}

pub struct Handler {
	core: Core,
	in_substreams: InSubstreams,
	out_substream: OutSubstream,
	/// [`KeepAlive::Until`] if idle, [`KeepAlive::Yes`] otherwise.
	keep_alive: KeepAlive,
}

impl Handler {
	pub fn new(peer_id: PeerId, block_provider: Arc<dyn BlockProvider>) -> Self {
		Self {
			core: Core::new(peer_id, block_provider),
			in_substreams: InSubstreams::new(),
			out_substream: OutSubstream::None,
			keep_alive: KeepAlive::Yes, // Will be set properly by the first poll call
		}
	}

	/// Returns `None` if `poll_step` should be called again immediately. Returns
	/// `Some(Poll::Pending)` if there is nothing more to do right now.
	fn poll_step(
		&mut self,
		cx: &mut Context<'_>,
	) -> Option<Poll<ConnectionHandlerEvent<ReadyUpgrade<&'static [u8]>, (), void::Void, Error>>> {
		if self.core.num_pending() < SOFT_MAX_PENDING {
			if let Poll::Ready(Some(message)) = self.in_substreams.poll_next_unpin(cx) {
				self.core.handle_message(message);
				self.keep_alive = KeepAlive::Yes; // Reset idle timeout
				return None
			}
		}

		match std::mem::replace(&mut self.out_substream, OutSubstream::None) {
			OutSubstream::None => {
				// Open a substream if we have something to send
				if self.core.any_pending() {
					self.out_substream = OutSubstream::Opening;
					return Some(Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest {
						protocol: SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL_NAME), ()),
					}))
				}
			},
			// Opening case handled by on_connection_event
			OutSubstream::Opening => self.out_substream = OutSubstream::Opening,
			OutSubstream::Idle(mut s) =>
				if let Some(message) = self.core.try_build_message() {
					self.out_substream = OutSubstream::Writing(Box::pin(async move {
						write_length_prefixed(&mut s, message).await?;
						Ok(s)
					}));
					return None
				} else {
					self.out_substream = OutSubstream::Idle(s);
				},
			OutSubstream::Writing(mut fut) => match fut.poll_unpin(cx) {
				Poll::Ready(Ok(s)) => {
					self.out_substream = OutSubstream::Idle(s);
					return None
				},
				Poll::Ready(Err(err)) =>
					return Some(Poll::Ready(ConnectionHandlerEvent::Close(Error::Io(err)))),
				Poll::Pending => self.out_substream = OutSubstream::Writing(fut),
			},
			OutSubstream::UpgradeError(err) =>
				return Some(Poll::Ready(ConnectionHandlerEvent::Close(Error::Upgrade(err)))),
		}

		Some(Poll::Pending)
	}
}

impl ConnectionHandler for Handler {
	type InEvent = void::Void;
	type OutEvent = void::Void;
	type Error = Error;
	type InboundProtocol = ReadyUpgrade<&'static [u8]>;
	type OutboundProtocol = ReadyUpgrade<&'static [u8]>;
	type InboundOpenInfo = ();
	type OutboundOpenInfo = ();

	fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
		SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL_NAME), ())
	}

	fn connection_keep_alive(&self) -> KeepAlive {
		self.keep_alive
	}

	fn poll(
		&mut self,
		cx: &mut Context<'_>,
	) -> Poll<
		ConnectionHandlerEvent<
			Self::OutboundProtocol,
			Self::OutboundOpenInfo,
			Self::OutEvent,
			Self::Error,
		>,
	> {
		let poll = loop {
			if let Some(poll) = self.poll_step(cx) {
				break poll
			}
		};

		if self.core.any_pending() || matches!(self.out_substream, OutSubstream::Writing(_)) {
			// Keep alive while we are sending a reply
			self.keep_alive = KeepAlive::Yes;
		} else if !matches!(self.keep_alive, KeepAlive::Until(_)) {
			// Not sending a reply. Keep alive for the idle timeout.
			self.keep_alive = KeepAlive::Until(Instant::now() + IDLE_KEEP_ALIVE);
		}

		poll
	}

	fn on_behaviour_event(&mut self, event: Self::InEvent) {
		void::unreachable(event);
	}

	fn on_connection_event(
		&mut self,
		event: ConnectionEvent<
			'_,
			Self::InboundProtocol,
			Self::OutboundProtocol,
			Self::InboundOpenInfo,
			Self::OutboundOpenInfo,
		>,
	) {
		match event {
			ConnectionEvent::FullyNegotiatedInbound(FullyNegotiatedInbound {
				protocol: s, ..
			}) => self.in_substreams.push(self.core.peer_id(), s),
			ConnectionEvent::FullyNegotiatedOutbound(FullyNegotiatedOutbound {
				protocol: s,
				..
			}) => self.out_substream = OutSubstream::Idle(s),
			ConnectionEvent::AddressChange(_) => (), // Don't care
			ConnectionEvent::DialUpgradeError(DialUpgradeError { error, .. }) =>
				self.out_substream = OutSubstream::UpgradeError(error),
			ConnectionEvent::ListenUpgradeError(ListenUpgradeError { error, .. }) => {
				debug!(
					target: LOG_TARGET,
					"Failed to open inbound substream from {}: {error}",
					self.core.peer_id(),
				);
			},
		}
	}
}
