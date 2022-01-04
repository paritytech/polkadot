// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Runtime declaration of the parachain metrics.

use polkadot_runtime_metrics::{Counter, CounterVec};
use primitives::v1::metric_definitions::{
	PARACHAIN_CREATE_INHERENT_BITFIELDS_SIGNATURE_CHECKS,
	PARACHAIN_INHERENT_DATA_BITFIELDS_PROCESSED, PARACHAIN_INHERENT_DATA_CANDIDATES_PROCESSED,
	PARACHAIN_INHERENT_DATA_DISPUTE_SETS_INCLUDED, PARACHAIN_INHERENT_DATA_DISPUTE_SETS_PROCESSED,
	PARACHAIN_INHERENT_DATA_WEIGHT,
};

/// Returns a runtime metric instance for sampling inherent data weight.
pub fn inherent_data_weight() -> CounterVec {
	CounterVec::new(PARACHAIN_INHERENT_DATA_WEIGHT)
}

/// Returns a runtime metric instance for recording how many inherent bitfields we process in
/// `enter_inner`.
pub fn bitfields_processed() -> Counter {
	Counter::new(PARACHAIN_INHERENT_DATA_BITFIELDS_PROCESSED)
}

/// Returns a runtime metric instance for recording how many parachain candidates we process in
/// `enter_inner`.
pub fn candidates_processed() -> CounterVec {
	CounterVec::new(PARACHAIN_INHERENT_DATA_CANDIDATES_PROCESSED)
}

/// Returns a runtime metric instance for counting dispute statements sets processed in
/// `enter_inner`.
pub fn dispute_sets_processed() -> CounterVec {
	CounterVec::new(PARACHAIN_INHERENT_DATA_DISPUTE_SETS_PROCESSED)
}

/// Returns a runtime metric instance for counting dispute statements sets included in
/// `enter_inner`.
pub fn disputes_included() -> Counter {
	Counter::new(PARACHAIN_INHERENT_DATA_DISPUTE_SETS_INCLUDED)
}

/// Returns a runtime metric instance for counting bitfield signature checks in
/// `enter_inner`.
pub fn bitfields_signature_checks() -> CounterVec {
	CounterVec::new(PARACHAIN_CREATE_INHERENT_BITFIELDS_SIGNATURE_CHECKS)
}
