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
use primitives::{vstaging::ExecutorParams, CandidateHash, DisputeState, SessionIndex};
use sp_std::prelude::*;

/// Implementation for `get_session_disputes` function from the runtime API
pub fn get_session_disputes<T: disputes::Config>(
) -> Vec<(SessionIndex, CandidateHash, DisputeState<T::BlockNumber>)> {
	<disputes::Pallet<T>>::disputes()
}

/// Get session executor parameter set
pub fn session_executor_params<T: session_info::Config>(
	session_index: SessionIndex,
) -> Option<ExecutorParams> {
	// This is to bootstrap the storage working around the runtime migration issue:
	// https://github.com/paritytech/substrate/issues/9997
	// After the bootstrap is complete (no less than 7 session passed with the runtime)
	// this code should be replaced with a pure
	// <session_info::Pallet<T>>::session_executor_params(session_index) call.
	match <session_info::Pallet<T>>::session_executor_params(session_index) {
		Some(ep) => Some(ep),
		None => Some(ExecutorParams::default()),
	}
}
