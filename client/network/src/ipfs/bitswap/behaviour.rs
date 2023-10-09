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

use super::{super::block_provider::BlockProvider, handler::Handler};
use libp2p::{
	core::connection::Endpoint,
	swarm::{
		behaviour::{FromSwarm, NetworkBehaviour, PollParameters, ToSwarm},
		ConnectionDenied, ConnectionId, THandlerInEvent, THandlerOutEvent,
	},
	Multiaddr, PeerId,
};
use std::{
	sync::Arc,
	task::{Context, Poll},
};

pub struct Behaviour {
	block_provider: Arc<dyn BlockProvider>,
}

impl Behaviour {
	pub fn new(block_provider: Arc<dyn BlockProvider>) -> Self {
		Self { block_provider }
	}
}

impl NetworkBehaviour for Behaviour {
	type ConnectionHandler = Handler;
	type OutEvent = void::Void;

	fn handle_established_inbound_connection(
		&mut self,
		_connection_id: ConnectionId,
		peer_id: PeerId,
		_local_addr: &Multiaddr,
		_remote_addr: &Multiaddr,
	) -> Result<Self::ConnectionHandler, ConnectionDenied> {
		Ok(Handler::new(peer_id, self.block_provider.clone()))
	}

	fn handle_established_outbound_connection(
		&mut self,
		_connection_id: ConnectionId,
		peer_id: PeerId,
		_addr: &Multiaddr,
		_role_override: Endpoint,
	) -> Result<Self::ConnectionHandler, ConnectionDenied> {
		Ok(Handler::new(peer_id, self.block_provider.clone()))
	}

	fn on_swarm_event(&mut self, _event: FromSwarm<'_, Self::ConnectionHandler>) {}

	fn on_connection_handler_event(
		&mut self,
		_peer_id: PeerId,
		_connection_id: ConnectionId,
		event: THandlerOutEvent<Self>,
	) {
		void::unreachable(event);
	}

	fn poll(
		&mut self,
		_cx: &mut Context<'_>,
		_params: &mut impl PollParameters,
	) -> Poll<ToSwarm<Self::OutEvent, THandlerInEvent<Self>>> {
		Poll::Pending
	}
}
