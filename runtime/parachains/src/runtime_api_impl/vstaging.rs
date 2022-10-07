// Copyright 2017-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Put implementations of functions from staging APIs here.

use crate::{disputes, session_info};
use primitives::v2::{CandidateHash, DisputeState, SessionIndex};
use primitives::vstaging::ExecutorParams;
use sp_std::prelude::*;

/// Implementation for `get_session_disputes` function from the runtime API
pub fn get_session_disputes<T: disputes::Config>(
) -> Vec<(SessionIndex, CandidateHash, DisputeState<T::BlockNumber>)> {
	<disputes::Pallet<T>>::disputes()
}

/// Get session executor parameter set by parent hash
pub fn session_ee_params_by_parent_hash<T: session_info::Config>(
	parent_hash: T::Hash,
) -> Option<ExecutorParams> {
	if let Some(session_index) = <session_info::Pallet<T>>::session_index_by_parent_hash(parent_hash) {
		<session_info::Pallet<T>>::session_ee_params(session_index)
	} else {
		None
	}
}
