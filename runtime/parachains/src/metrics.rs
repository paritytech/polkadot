// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Declaration of the parachain specific runtime metrics.

use polkadot_runtime_metrics::{Counter, CounterVec};

/// Returns a runtime metric instance for sampling inherent data weight.
pub fn inherent_data_weight() -> CounterVec {
	CounterVec::new(
		"parachain_inherent_data_weight",
		"Inherent data weight before and after filtering",
		&["when"],
	)
}
/// Returns a runtime metric instance for recording how many inherent bitfields we process in
/// `enter_inner`.
pub fn bitfields_processed() -> Counter {
	Counter::new(
		"parachain_inherent_data_bitfields_processed",
		"Counts the number of bitfields processed in `enter_inner`.",
	)
}

/// Returns a runtime metric instance for recording how many parachain candidates we process in
/// `enter_inner`.
pub fn candidates_processed() -> CounterVec {
	CounterVec::new(
		"parachain_inherent_data_candidates_processed",
		"Counts the number of parachain block candidates processed in `enter_inner`.",
		&["category"],
	)
}

/// Returns a runtime metric instance for counting dispute statements sets processed in
/// `enter_inner`.
pub fn dispute_sets_processed() -> CounterVec {
	CounterVec::new(
		"parachain_inherent_data_dispute_sets_processed",
		"Counts the number of dispute statements sets processed in `enter_inner`.",
		&["category"],
	)
}

/// Returns a runtime metric instance for counting dispute statements sets included in
/// `enter_inner`.
pub fn disputes_included() -> Counter {
	Counter::new(
		"parachain_inherent_data_dispute_sets_included",
		"Counts the number of dispute statements sets included in a block in `enter_inner`.",
	)
}

/// Returns a runtime metric instance for counting bitfield signature checks in
/// `enter_inner`.
pub fn bitfields_signature_checks() -> CounterVec {
	CounterVec::new(
		"create_inherent_bitfields_signature_checks",
		"Counts the number of bitfields signature checked in `enter_inner`.",
		&["validity"],
	)
}
