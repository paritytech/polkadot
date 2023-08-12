// Copyright (C) Parity Technologies (UK) Ltd.
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

use polkadot_runtime_metrics::{Counter, CounterVec, Histogram};
use primitives::metric_definitions::{
	PARACHAIN_CREATE_INHERENT_BITFIELDS_SIGNATURE_CHECKS,
	PARACHAIN_INHERENT_DATA_BITFIELDS_PROCESSED, PARACHAIN_INHERENT_DATA_CANDIDATES_PROCESSED,
	PARACHAIN_INHERENT_DATA_DISPUTE_SETS_PROCESSED, PARACHAIN_INHERENT_DATA_WEIGHT,
	PARACHAIN_VERIFY_DISPUTE_SIGNATURE,
};

pub struct Metrics {
	/// Samples inherent data weight.
	inherent_data_weight: CounterVec,
	/// Counts how many inherent bitfields processed in `process_inherent_data`.
	bitfields_processed: Counter,
	/// Counts how many parachain candidates processed in `process_inherent_data`.
	candidates_processed: CounterVec,
	/// Counts dispute statements sets processed in `process_inherent_data`.
	dispute_sets_processed: CounterVec,
	/// Counts bitfield signature checks in `process_inherent_data`.
	bitfields_signature_checks: CounterVec,

	/// Histogram with the time spent checking a validator signature of a dispute statement
	signature_timings: Histogram,
}

impl Metrics {
	/// Sample the inherent data weight metric before filtering.
	pub fn on_before_filter(&self, value: u64) {
		self.inherent_data_weight.with_label_values(&["before-filter"]).inc_by(value);
	}

	/// Sample the inherent data weight metric after filtering.
	pub fn on_after_filter(&self, value: u64) {
		self.inherent_data_weight.with_label_values(&["after-filter"]).inc_by(value);
	}

	/// Increment the number of bitfields processed.
	pub fn on_bitfields_processed(&self, value: u64) {
		self.bitfields_processed.inc_by(value);
	}

	/// Increment the number of parachain candidates included.
	pub fn on_candidates_included(&self, value: u64) {
		self.candidates_processed.with_label_values(&["included"]).inc_by(value);
	}

	/// Increment the number of parachain candidates sanitized.
	pub fn on_candidates_sanitized(&self, value: u64) {
		self.candidates_processed.with_label_values(&["sanitized"]).inc_by(value);
	}

	/// Increment the total number of parachain candidates received in `process_inherent_data`.
	pub fn on_candidates_processed_total(&self, value: u64) {
		self.candidates_processed.with_label_values(&["total"]).inc_by(value);
	}

	/// Sample the relay chain freeze events causing runtime to not process candidates in
	/// `process_inherent_data`.
	pub fn on_relay_chain_freeze(&self) {
		self.dispute_sets_processed.with_label_values(&["frozen"]).inc();
	}

	/// Increment the number of disputes that have concluded as invalid.
	pub fn on_disputes_concluded_invalid(&self, value: u64) {
		self.dispute_sets_processed
			.with_label_values(&["concluded_invalid"])
			.inc_by(value);
	}

	/// Increment the number of disputes imported.
	pub fn on_disputes_imported(&self, value: u64) {
		self.dispute_sets_processed.with_label_values(&["imported"]).inc_by(value);
	}

	pub fn on_valid_bitfield_signature(&self) {
		self.bitfields_signature_checks.with_label_values(&["valid"]).inc_by(1);
	}

	pub fn on_invalid_bitfield_signature(&self) {
		self.bitfields_signature_checks.with_label_values(&["invalid"]).inc_by(1);
	}

	pub fn on_signature_check_complete(&self, val: u128) {
		self.signature_timings.observe(val);
	}
}

pub const METRICS: Metrics = Metrics {
	inherent_data_weight: CounterVec::new(PARACHAIN_INHERENT_DATA_WEIGHT),
	bitfields_processed: Counter::new(PARACHAIN_INHERENT_DATA_BITFIELDS_PROCESSED),
	candidates_processed: CounterVec::new(PARACHAIN_INHERENT_DATA_CANDIDATES_PROCESSED),
	dispute_sets_processed: CounterVec::new(PARACHAIN_INHERENT_DATA_DISPUTE_SETS_PROCESSED),
	bitfields_signature_checks: CounterVec::new(
		PARACHAIN_CREATE_INHERENT_BITFIELDS_SIGNATURE_CHECKS,
	),
	signature_timings: Histogram::new(PARACHAIN_VERIFY_DISPUTE_SIGNATURE),
};
