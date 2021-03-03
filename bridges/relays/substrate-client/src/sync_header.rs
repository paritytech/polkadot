// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use headers_relay::sync_types::SourceHeader;
use num_traits::{CheckedSub, One};
use relay_utils::HeaderId;
use sp_runtime::traits::Header as HeaderT;

/// Generic wrapper for `sp_runtime::traits::Header` based headers, that
/// implements `headers_relay::sync_types::SourceHeader` and may be used in headers sync directly.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncHeader<Header>(Header);

impl<Header> SyncHeader<Header> {
	/// Extracts wrapped header from self.
	pub fn into_inner(self) -> Header {
		self.0
	}
}

impl<Header> std::ops::Deref for SyncHeader<Header> {
	type Target = Header;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<Header> From<Header> for SyncHeader<Header> {
	fn from(header: Header) -> Self {
		Self(header)
	}
}

impl<Header: HeaderT> SourceHeader<Header::Hash, Header::Number> for SyncHeader<Header> {
	fn id(&self) -> HeaderId<Header::Hash, Header::Number> {
		relay_utils::HeaderId(*self.number(), self.hash())
	}

	fn parent_id(&self) -> HeaderId<Header::Hash, Header::Number> {
		relay_utils::HeaderId(
			self.number()
				.checked_sub(&One::one())
				.expect("should never be called for genesis header"),
			*self.parent_hash(),
		)
	}
}
