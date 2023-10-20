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

use super::{
	super::config::MultiaddrWithPeerId,
	block_provider::{BlockProvider, Change as BlockProviderChange},
};
use futures::{FutureExt, Stream};
use futures_timer::Delay;
use ip_network::IpNetwork;
use libp2p::{
	core::connection::Endpoint,
	kad::{record::store::MemoryStore, Kademlia, RoutingUpdate},
	multiaddr::Protocol,
	swarm::{
		behaviour::{FromSwarm, NetworkBehaviour, NewExternalAddr, PollParameters, ToSwarm},
		ConnectionDenied, ConnectionId, THandler, THandlerInEvent, THandlerOutEvent,
	},
	Multiaddr, PeerId,
};
use log::{debug, warn};
use std::{
	pin::Pin,
	sync::Arc,
	task::{Context, Poll},
	time::Duration,
};

const LOG_TARGET: &str = "ipfs::dht";

/// The period between DHT "bootstraps".
// Regular bootstrapping is recommended here:
// https://github.com/libp2p/rust-libp2p/issues/2122#issuecomment-875050447
const BOOTSTRAP_PERIOD: Duration = Duration::from_secs(5 * 60);

fn is_global_addr(addr: &Multiaddr) -> bool {
	let ip: IpNetwork = match addr.iter().next() {
		Some(Protocol::Ip4(ip)) => ip.into(),
		Some(Protocol::Ip6(ip)) => ip.into(),
		Some(Protocol::Dns(_)) | Some(Protocol::Dns4(_)) | Some(Protocol::Dns6(_)) => return true,
		_ => return false,
	};
	ip.is_global()
}

enum State {
	/// We are waiting for a global external address to be provided. We don't attempt to bootstrap
	/// or publish anything until we have forwarded such an address to the [`Kademlia`] instance.
	WaitingForAddr { block_provider: Arc<dyn BlockProvider> },
	/// Normal operation.
	Ready {
		next_bootstrap_delay: Delay,
		block_provider_changes: Pin<Box<dyn Stream<Item = BlockProviderChange> + Send>>,
	},
	/// Something went very wrong. It is not possible to recover from this state.
	Dead,
}

pub struct Behaviour {
	kad: Kademlia<MemoryStore>,
	state: State,
}

impl Behaviour {
	pub fn new(
		boot_nodes: &[MultiaddrWithPeerId],
		local_peer_id: PeerId,
		block_provider: Arc<dyn BlockProvider>,
	) -> Self {
		let mut kad = Kademlia::new(local_peer_id, MemoryStore::new(local_peer_id));

		for boot_node in boot_nodes {
			if matches!(
				kad.add_address(&boot_node.peer_id, boot_node.multiaddr.clone()),
				RoutingUpdate::Failed
			) {
				warn!(
					target: LOG_TARGET,
					"Failed to add boot node {} with address {}",
					boot_node.peer_id, boot_node.multiaddr
				);
			}
		}

		Self { kad, state: State::WaitingForAddr { block_provider } }
	}

	/// Add a self-reported address of a remote peer to the k-buckets of the DHT if it has
	/// compatible `supported_protocols`.
	pub fn add_self_reported_address(
		&mut self,
		peer_id: &PeerId,
		supported_protocols: &[impl AsRef<[u8]>],
		addr: &Multiaddr,
	) {
		// Add to DHT if address is global and peer supports the DHT protocol
		if is_global_addr(addr) &&
			supported_protocols
				.iter()
				.any(|a| self.kad.protocol_names().iter().any(|b| a.as_ref() == b.as_ref()))
		{
			self.kad.add_address(peer_id, addr.clone());
		}
	}
}

impl NetworkBehaviour for Behaviour {
	type ConnectionHandler = <Kademlia<MemoryStore> as NetworkBehaviour>::ConnectionHandler;
	type OutEvent = void::Void;

	fn on_swarm_event(&mut self, event: FromSwarm<'_, Self::ConnectionHandler>) {
		if let (
			State::WaitingForAddr { block_provider },
			FromSwarm::NewExternalAddr(NewExternalAddr { addr }),
		) = (&self.state, &event)
		{
			if is_global_addr(addr) {
				self.state = State::Ready {
					next_bootstrap_delay: Delay::new(Duration::ZERO),
					block_provider_changes: block_provider.changes(),
				};
			}
		}

		self.kad.on_swarm_event(event);
	}

	fn on_connection_handler_event(
		&mut self,
		peer_id: PeerId,
		connection_id: ConnectionId,
		event: THandlerOutEvent<Self>,
	) {
		self.kad.on_connection_handler_event(peer_id, connection_id, event);
	}

	fn poll(
		&mut self,
		cx: &mut Context<'_>,
		params: &mut impl PollParameters,
	) -> Poll<ToSwarm<Self::OutEvent, THandlerInEvent<Self>>> {
		if let State::Ready { next_bootstrap_delay, block_provider_changes } = &mut self.state {
			if next_bootstrap_delay.poll_unpin(cx).is_ready() {
				if let Err(err) = self.kad.bootstrap() {
					warn!(target: LOG_TARGET, "Bootstrapping failed: {err}");
				}
				loop {
					next_bootstrap_delay.reset(BOOTSTRAP_PERIOD);
					if next_bootstrap_delay.poll_unpin(cx).is_pending() {
						break
					}
				}
			}

			loop {
				match block_provider_changes.as_mut().poll_next(cx) {
					Poll::Ready(Some(BlockProviderChange::Added(multihash))) =>
						if let Err(err) = self.kad.start_providing(multihash.into()) {
							debug!(target: LOG_TARGET, "Failed to add {multihash:?} to DHT: {err}");
						},
					Poll::Ready(Some(BlockProviderChange::Removed(multihash))) =>
						self.kad.stop_providing(&multihash.into()),
					Poll::Ready(None) => {
						self.state = State::Dead;
						break
					},
					Poll::Pending => break,
				}
			}
		}

		loop {
			break match self.kad.poll(cx, params) {
				Poll::Ready(ToSwarm::GenerateEvent(_)) => continue,
				Poll::Ready(ToSwarm::Dial { opts }) => Poll::Ready(ToSwarm::Dial { opts }),
				Poll::Ready(ToSwarm::NotifyHandler { peer_id, handler, event }) =>
					Poll::Ready(ToSwarm::NotifyHandler { peer_id, handler, event }),
				Poll::Ready(ToSwarm::ReportObservedAddr { address, score }) =>
					Poll::Ready(ToSwarm::ReportObservedAddr { address, score }),
				Poll::Ready(ToSwarm::CloseConnection { peer_id, connection }) =>
					Poll::Ready(ToSwarm::CloseConnection { peer_id, connection }),
				Poll::Pending => Poll::Pending,
			}
		}
	}

	fn handle_pending_inbound_connection(
		&mut self,
		connection_id: ConnectionId,
		local_addr: &Multiaddr,
		remote_addr: &Multiaddr,
	) -> Result<(), ConnectionDenied> {
		self.kad
			.handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
	}

	fn handle_established_inbound_connection(
		&mut self,
		connection_id: ConnectionId,
		peer_id: PeerId,
		local_addr: &Multiaddr,
		remote_addr: &Multiaddr,
	) -> Result<THandler<Self>, ConnectionDenied> {
		self.kad.handle_established_inbound_connection(
			connection_id,
			peer_id,
			local_addr,
			remote_addr,
		)
	}

	fn handle_pending_outbound_connection(
		&mut self,
		connection_id: ConnectionId,
		maybe_peer_id: Option<PeerId>,
		addrs: &[Multiaddr],
		effective_role: Endpoint,
	) -> Result<Vec<Multiaddr>, ConnectionDenied> {
		self.kad.handle_pending_outbound_connection(
			connection_id,
			maybe_peer_id,
			addrs,
			effective_role,
		)
	}

	fn handle_established_outbound_connection(
		&mut self,
		connection_id: ConnectionId,
		peer_id: PeerId,
		addr: &Multiaddr,
		role_override: Endpoint,
	) -> Result<THandler<Self>, ConnectionDenied> {
		self.kad
			.handle_established_outbound_connection(connection_id, peer_id, addr, role_override)
	}
}
