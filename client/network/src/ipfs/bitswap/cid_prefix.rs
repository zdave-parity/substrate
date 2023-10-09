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

use cid::{Cid, Version};
use unsigned_varint::encode as varint_encode;

/// All the metadata of a CID, without the actual digest.
pub struct CidPrefix {
	version: Version,
	codec: u64,
	hash_code: u64,
	hash_size: u8,
}

impl CidPrefix {
	/// Returns the encoded bytes of the CID prefix.
	pub fn to_bytes(&self) -> Vec<u8> {
		let mut res = Vec::with_capacity(8);
		let mut buf = varint_encode::u64_buffer();
		if self.version != Version::V0 {
			res.extend_from_slice(varint_encode::u64(self.version.into(), &mut buf));
			res.extend_from_slice(varint_encode::u64(self.codec, &mut buf));
		}
		res.extend_from_slice(varint_encode::u64(self.hash_code, &mut buf));
		res.extend_from_slice(varint_encode::u64(self.hash_size.into(), &mut buf));
		res
	}
}

impl From<&Cid> for CidPrefix {
	fn from(cid: &Cid) -> Self {
		Self {
			version: cid.version(),
			codec: cid.codec(),
			hash_code: cid.hash().code(),
			hash_size: cid.hash().size(),
		}
	}
}
