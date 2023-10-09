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
	super::block_provider::BlockProvider,
	cid_prefix::CidPrefix,
	schema::bitswap::{
		message::{wantlist::WantType, Block, BlockPresence, BlockPresenceType},
		Message,
	},
};
use cid::Cid;
use hashlink::{LinkedHashMap, LinkedHashSet};
use libp2p::PeerId;
use log::debug;
use prost::Message as ProstMessage;
use std::sync::Arc;

const LOG_TARGET: &str = "ipfs::bitswap";

// Note that each outbound message either contains a list of block presences _or_ a list of blocks
// (this is an implementation choice, it is not required by the specification)
const MAX_PRESENCES_PER_OUT_MESSAGE: usize = 100;
const MAX_BLOCKS_PER_OUT_MESSAGE: usize = 1;

pub struct Core {
	peer_id: PeerId,
	block_provider: Arc<dyn BlockProvider>,
	/// Queue of block presences to send (presences at the front should be sent first). The `bool`
	/// for a CID is `true` if we have the block (this information may be outdated by the time the
	/// presence is popped, but that doesn't really matter).
	pending_presences: LinkedHashMap<Cid, bool>,
	/// Queue of blocks to send (blocks at the front should be sent first). Note that we may not
	/// have these blocks, as they may have disappeared from the block provider since being pushed
	/// onto the queue.
	pending_blocks: LinkedHashSet<Cid>,
}

impl Core {
	pub fn new(peer_id: PeerId, block_provider: Arc<dyn BlockProvider>) -> Self {
		Self {
			peer_id,
			block_provider,
			pending_presences: LinkedHashMap::new(),
			pending_blocks: LinkedHashSet::new(),
		}
	}

	pub fn peer_id(&self) -> &PeerId {
		&self.peer_id
	}

	/// Returns the number of pending blocks/presences.
	pub fn num_pending(&self) -> usize {
		self.pending_presences.len().saturating_add(self.pending_blocks.len())
	}

	/// Returns `true` if there are any pending blocks/presences.
	pub fn any_pending(&self) -> bool {
		!self.pending_presences.is_empty() || !self.pending_blocks.is_empty()
	}

	/// Handle an inbound message.
	pub fn handle_message(&mut self, message: Vec<u8>) {
		let message = match Message::decode(message.as_slice()) {
			Ok(message) => message,
			Err(err) => {
				debug!(
					target: LOG_TARGET,
					"Error decoding message from {}: {err}",
					self.peer_id,
				);
				return
			},
		};

		let Some(wantlist) = message.wantlist else {
			debug!(
				target: LOG_TARGET,
				"Inbound message from {} without wantlist",
				self.peer_id,
			);
			return
		};

		// If this is the full wantlist, clear any old pending stuff...
		if wantlist.full {
			self.pending_presences.clear();
			self.pending_blocks.clear();
		}

		for entry in wantlist.entries {
			let cid = match cid::Cid::read_bytes(entry.block.as_slice()) {
				Ok(cid) => cid,
				Err(err) => {
					debug!(
						target: LOG_TARGET,
						"Bad CID {:?} from {}: {err}",
						entry.block, self.peer_id,
					);
					continue
				},
			};

			if entry.cancel {
				self.pending_presences.remove(&cid);
				self.pending_blocks.remove(&cid);
			} else {
				// TODO Currently ignoring priority
				match WantType::from_i32(entry.want_type) {
					Some(WantType::Block) => {
						if self.block_provider.have(cid.hash()) {
							// If this block has already been requested, leave it where it is in
							// the queue
							self.pending_blocks.replace(cid);
						} else {
							debug!(
								target: LOG_TARGET,
								"Block {cid} requested by {} not found",
								self.peer_id,
							);
						}
					},
					Some(WantType::Have) => {
						let have = self.block_provider.have(cid.hash());
						if have || entry.send_dont_have {
							// If this block presence has already been requested, leave it where it
							// is in the queue
							self.pending_presences.replace(cid, have);
						}
					},
					None => debug!(
						target: LOG_TARGET,
						"Unrecognised want type {} from {}",
						entry.want_type, self.peer_id,
					),
				}
			}
		}
	}

	/// Try to build an outbound message.
	pub fn try_build_message(&mut self) -> Option<Vec<u8>> {
		let mut message = Message {
			wantlist: None,
			blocks: Default::default(),
			payload: Default::default(),
			block_presences: Default::default(),
			pending_bytes: 0,
		};

		while message.block_presences.len() < MAX_PRESENCES_PER_OUT_MESSAGE {
			if let Some((cid, have)) = self.pending_presences.pop_front() {
				let presence_type =
					if have { BlockPresenceType::Have } else { BlockPresenceType::DontHave };
				message
					.block_presences
					.push(BlockPresence { cid: cid.to_bytes(), r#type: presence_type.into() });
			} else {
				break
			}
		}

		if message.block_presences.is_empty() {
			while message.blocks.len() < MAX_BLOCKS_PER_OUT_MESSAGE {
				if let Some(cid) = self.pending_blocks.pop_front() {
					if let Some(data) = self.block_provider.get(cid.hash()) {
						message
							.payload
							.push(Block { prefix: CidPrefix::from(&cid).to_bytes(), data });
					} else {
						debug!(
							target: LOG_TARGET,
							"Block {cid} has disappeared, cannot send to {}",
							self.peer_id,
						);
					}
				} else {
					break
				}
			}
		}

		if message.block_presences.is_empty() && message.payload.is_empty() {
			None
		} else {
			Some(message.encode_to_vec())
		}
	}
}
