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

//! Storage primitives for the Substrate light client (a.k.a bridge) pallet.

use bp_header_chain::AuthoritySet;
use codec::{Decode, Encode};
use core::default::Default;
#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};
use sp_finality_grandpa::{AuthorityList, SetId};
use sp_runtime::traits::Header as HeaderT;
use sp_runtime::RuntimeDebug;

/// Data required for initializing the bridge pallet.
///
/// The bridge needs to know where to start its sync from, and this provides that initial context.
#[derive(Default, Encode, Decode, RuntimeDebug, PartialEq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct InitializationData<H: HeaderT> {
	/// The header from which we should start syncing.
	pub header: H,
	/// The initial authorities of the pallet.
	pub authority_list: AuthorityList,
	/// The ID of the initial authority set.
	pub set_id: SetId,
	/// The first scheduled authority set change of the pallet.
	pub scheduled_change: Option<ScheduledChange<H::Number>>,
	/// Should the pallet block transaction immediately after initialization.
	pub is_halted: bool,
}

/// Keeps track of when the next GRANDPA authority set change will occur.
#[derive(Default, Encode, Decode, RuntimeDebug, PartialEq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct ScheduledChange<N> {
	/// The authority set that will be used once this change is enacted.
	pub authority_set: AuthoritySet,
	/// The block height at which the authority set should be enacted.
	///
	/// Note: It will only be enacted once a header at this height is finalized.
	pub height: N,
}

/// A more useful representation of a header for storage purposes.
#[derive(Default, Encode, Decode, Clone, RuntimeDebug, PartialEq)]
pub struct ImportedHeader<H: HeaderT> {
	/// A plain Substrate header.
	pub header: H,
	/// Does this header enact a new authority set change. If it does
	/// then it will require a justification.
	pub requires_justification: bool,
	/// Has this header been finalized, either explicitly via a justification,
	/// or implicitly via one of its children getting finalized.
	pub is_finalized: bool,
	/// The hash of the header which scheduled a change on this fork. If there are currently
	/// not pending changes on this fork this will be empty.
	pub signal_hash: Option<H::Hash>,
}

impl<H: HeaderT> core::ops::Deref for ImportedHeader<H> {
	type Target = H;

	fn deref(&self) -> &H {
		&self.header
	}
}
