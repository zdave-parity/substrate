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

use cid::multihash::Multihash;
use core::marker::PhantomData;
use log::debug;
use sc_client_api::BlockBackend;
use sp_runtime::traits::{BlakeTwo256, Block, Hash, Header};
use std::sync::Arc;

const LOG_TARGET: &str = "ipfs";

/// Provides blocks to be served over IPFS. Requires `Send` and `Sync` so we can write `Arc<dyn
/// BlockProvider>` instead of `Arc<dyn BlockProvider + Send + Sync>`.
pub trait BlockProvider: Send + Sync {
	/// Returns `true` if we have the block with the given hash.
	fn have(&self, multihash: &Multihash) -> bool;

	/// Returns the block with the given hash if possible, otherwise returns `None`.
	fn get(&self, multihash: &Multihash) -> Option<Vec<u8>>;
}

/// Implemented for hasher types such as [`BlakeTwo256`], providing the corresponding Multihash
/// code.
pub trait HasMultihashCode {
	/// The Multihash code for the hasher.
	const MULTIHASH_CODE: u64;
}

impl HasMultihashCode for BlakeTwo256 {
	const MULTIHASH_CODE: u64 = 0xb220;
}

fn try_from_multihash<H: Hash + HasMultihashCode>(multihash: &Multihash) -> Option<H::Output> {
	if multihash.code() != H::MULTIHASH_CODE {
		return None
	}
	let mut hash = H::Output::default();
	let src = multihash.digest();
	let dst = hash.as_mut();
	if src.len() != dst.len() {
		return None
	}
	dst.copy_from_slice(src);
	Some(hash)
}

/// Implements [`BlockProvider`], providing access to indexed transactions in the wrapped client.
/// Note that it isn't possible to just implement [`BlockProvider`] on types implementing
/// [`BlockBackend`] because `BlockBackend` is generic over the (chain) block type.
pub struct IndexedTransactions<B, C> {
	client: Arc<C>,
	phantom: PhantomData<B>,
}

impl<B, C> IndexedTransactions<B, C> {
	/// Create a new `IndexedTransactions` wrapper over the given client.
	pub fn new(client: Arc<C>) -> Self {
		Self { client, phantom: PhantomData }
	}
}

impl<H, B, C> BlockProvider for IndexedTransactions<B, C>
where
	H: Hash + HasMultihashCode,
	B: Block<Hash = H::Output>,
	B::Header: Header<Hashing = H>,
	C: BlockBackend<B> + Send + Sync,
{
	fn have(&self, multihash: &Multihash) -> bool {
		let Some(hash) = try_from_multihash::<H>(multihash) else { return false };
		match self.client.has_indexed_transaction(hash) {
			Ok(have) => have,
			Err(err) => {
				debug!(target: LOG_TARGET, "Error checking for block {hash:?}: {err}");
				false
			},
		}
	}

	fn get(&self, multihash: &Multihash) -> Option<Vec<u8>> {
		let Some(hash) = try_from_multihash::<H>(multihash) else { return None };
		match self.client.indexed_transaction(hash) {
			Ok(block) => block,
			Err(err) => {
				debug!(target: LOG_TARGET, "Error getting block {hash:?}: {err}");
				None
			},
		}
	}
}
