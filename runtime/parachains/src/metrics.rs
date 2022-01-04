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

pub struct Metrics {
	/// Samples inherent data weight.
	inherent_data_weight: CounterVec,
	/// Counts how many inherent bitfields processed in `enter_inner`.
	bitfields_processed: Counter,
	/// Counts how many parachain candidates processed in `enter_inner`.
	candidates_processed: CounterVec,
	/// Counts dispute statements sets processed in `enter_inner`.
	dispute_sets_processed: CounterVec,
	/// Counts dispute statements sets included in `enter_inner`.
	disputes_included: Counter,
	/// Counts bitfield signature checks in `enter_inner`.
	bitfields_signature_checks: CounterVec,
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

	/// Increment the total number of parachain candidates received in `enter_inner`.
	pub fn on_candidates_processed_total(&self, value: u64) {
		self.candidates_processed.with_label_values(&["total"]).inc_by(value);
	}

	/// Sample the relay chain freeze events causing runtime to not process candidates in
	/// `enter_inner`.
	pub fn on_relay_chain_freeze(&self) {
		self.dispute_sets_processed.with_label_values(&["frozen"]).inc();
	}

	/// Sample the number of dispute sets processed from the current session.
	pub fn on_current_session_disputes_processed(&self, value: u64) {
		self.dispute_sets_processed.with_label_values(&["current"]).inc_by(value);
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

	pub fn on_disputes_included(&self, value: u64) {
		self.disputes_included.inc_by(value);
	}

	pub fn on_valid_bitfield_signature(&self) {
		self.bitfields_signature_checks.with_label_values(&["valid"]).inc();
	}

	pub fn on_invalid_bitfield_signature(&self) {
		self.bitfields_signature_checks.with_label_values(&["invalid"]).inc();
	}
}

pub const METRICS: Metrics = Metrics {
	inherent_data_weight: CounterVec::new(PARACHAIN_INHERENT_DATA_WEIGHT),
	bitfields_processed: Counter::new(PARACHAIN_INHERENT_DATA_BITFIELDS_PROCESSED),
	candidates_processed: CounterVec::new(PARACHAIN_INHERENT_DATA_CANDIDATES_PROCESSED),
	dispute_sets_processed: CounterVec::new(PARACHAIN_INHERENT_DATA_DISPUTE_SETS_PROCESSED),
	disputes_included: Counter::new(PARACHAIN_INHERENT_DATA_DISPUTE_SETS_INCLUDED),
	bitfields_signature_checks: CounterVec::new(
		PARACHAIN_CREATE_INHERENT_BITFIELDS_SIGNATURE_CHECKS,
	),
};
