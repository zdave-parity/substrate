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

use futures::stream::{SelectAll, Stream, StreamExt};
use libp2p::{core::upgrade::read_length_prefixed, swarm::NegotiatedSubstream, PeerId};
use log::debug;
use pin_project::pin_project;
use std::{
	future::Future,
	pin::Pin,
	task::{Context, Poll},
};

const LOG_TARGET: &str = "ipfs::bitswap";

/// Maximum number of inbound substreams open at the same time on one connection. We simply reset
/// any new inbound substreams once this limit is reached.
const MAX_SUBSTREAMS: usize = 4;

/// Maximum size of any inbound message. If a larger message is sent on an inbound substream, the
/// substream will simply be reset.
// The Bitswap spec says "all protocol messages must be less than or equal to 4MiB in size". This
// seems excessive for inbound messages though, given that noone should be sending us blocks.
// Restrict the maximum message size to avoid large allocations.
const MAX_MESSAGE_SIZE: usize = 32 * 1024;

async fn read_message(
	mut s: NegotiatedSubstream,
) -> std::io::Result<(NegotiatedSubstream, Vec<u8>)> {
	let message = read_length_prefixed(&mut s, MAX_MESSAGE_SIZE).await?;
	Ok((s, message))
}

#[pin_project]
struct Substream<R, F> {
	peer_id: PeerId,
	read_message: R,
	#[pin]
	next_message: F,
}

impl<R, F> Stream for Substream<R, F>
where
	R: Fn(NegotiatedSubstream) -> F,
	F: Future<Output = std::io::Result<(NegotiatedSubstream, Vec<u8>)>>,
{
	type Item = Vec<u8>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let mut this = self.project();
		match this.next_message.as_mut().poll(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(Err(err)) => {
				debug!(
					target: LOG_TARGET,
					"Error on inbound substream from {}, resetting: {err}",
					this.peer_id,
				);
				Poll::Ready(None)
			},
			Poll::Ready(Ok((s, message))) => {
				this.next_message.set((this.read_message)(s));
				Poll::Ready(Some(message))
			},
		}
	}
}

pub struct InSubstreams(SelectAll<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>>);

impl InSubstreams {
	pub fn new() -> Self {
		Self(SelectAll::new())
	}

	pub fn push(&mut self, peer_id: &PeerId, s: NegotiatedSubstream) {
		if self.0.len() >= MAX_SUBSTREAMS {
			debug!(
				target: LOG_TARGET,
				"Already at inbound substream limit; resetting new substream from {peer_id}",
			);
			return
		}
		let next_message = read_message(s);
		self.0
			.push(Box::pin(Substream { peer_id: *peer_id, read_message, next_message }));
	}
}

impl Stream for InSubstreams {
	type Item = Vec<u8>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.0.poll_next_unpin(cx)
	}
}
