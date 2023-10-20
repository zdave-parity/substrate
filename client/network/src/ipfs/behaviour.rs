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
	bitswap::Behaviour as BitswapBehaviour, block_provider::BlockProvider, config::Config,
	dht::Behaviour as DhtBehaviour,
};
use libp2p::{swarm::NetworkBehaviour, Multiaddr, PeerId};
use std::sync::Arc;

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "void::Void")]
pub struct Behaviour {
	bitswap: BitswapBehaviour,
	dht: DhtBehaviour,
}

impl Behaviour {
	pub fn new(
		config: Config,
		local_peer_id: PeerId,
		block_provider: Arc<dyn BlockProvider>,
	) -> Self {
		Self {
			bitswap: BitswapBehaviour::new(block_provider.clone()),
			dht: DhtBehaviour::new(&config.boot_nodes, local_peer_id, block_provider),
		}
	}

	/// Add a self-reported address of a remote peer to the k-buckets of the DHT if it has
	/// compatible `supported_protocols`.
	pub fn add_self_reported_address(
		&mut self,
		peer_id: &PeerId,
		supported_protocols: &[impl AsRef<[u8]>],
		addr: &Multiaddr,
	) {
		self.dht.add_self_reported_address(peer_id, supported_protocols, addr);
	}
}
