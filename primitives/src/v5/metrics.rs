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

//! Runtime metric primitives.

use parity_scale_codec::{Decode, Encode};
use sp_std::prelude::*;

/// Runtime metric operations.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum RuntimeMetricOp {
	/// Increment a counter metric with labels by value.
	IncrementCounterVec(u64, RuntimeMetricLabelValues),
	/// Increment a counter metric by value.
	IncrementCounter(u64),
	/// Observe histogram value
	ObserveHistogram(u128),
}

/// Runtime metric update event.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricUpdate {
	/// The name of the metric.
	pub metric_name: Vec<u8>,
	/// The operation applied to the metric.
	pub op: RuntimeMetricOp,
}

fn vec_to_str<'a>(v: &'a Vec<u8>, default: &'static str) -> &'a str {
	return sp_std::str::from_utf8(v).unwrap_or(default)
}

impl RuntimeMetricLabels {
	/// Returns a labels as `Vec<&str>`.
	pub fn as_str_vec(&self) -> Vec<&str> {
		self.0
			.iter()
			.map(|label_vec| vec_to_str(&label_vec.0, "invalid_label"))
			.collect()
	}

	/// Return the inner values as vec.
	pub fn clear(&mut self) {
		self.0.clear();
	}
}

impl From<&[&'static str]> for RuntimeMetricLabels {
	fn from(v: &[&'static str]) -> RuntimeMetricLabels {
		RuntimeMetricLabels(
			v.iter().map(|label| RuntimeMetricLabel(label.as_bytes().to_vec())).collect(),
		)
	}
}

impl RuntimeMetricUpdate {
	/// Returns the metric name.
	pub fn metric_name(&self) -> &str {
		vec_to_str(&self.metric_name, "invalid_metric_name")
	}
}

/// A set of metric labels.
#[derive(Clone, Default, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricLabels(Vec<RuntimeMetricLabel>);

/// A metric label.
#[derive(Clone, Default, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricLabel(Vec<u8>);

/// A metric label value.
pub type RuntimeMetricLabelValue = RuntimeMetricLabel;

/// A set of metric label values.
pub type RuntimeMetricLabelValues = RuntimeMetricLabels;

/// Trait for converting Vec<u8> to `&str`.
pub trait AsStr {
	/// Return a str reference.
	fn as_str(&self) -> Option<&str>;
}

impl AsStr for RuntimeMetricLabel {
	fn as_str(&self) -> Option<&str> {
		sp_std::str::from_utf8(&self.0).ok()
	}
}

impl From<&'static str> for RuntimeMetricLabel {
	fn from(s: &'static str) -> Self {
		Self(s.as_bytes().to_vec())
	}
}

/// Contains all runtime metrics defined as constants.
pub mod metric_definitions {
	/// `Counter` metric definition.
	pub struct CounterDefinition {
		/// The name of the metric.
		pub name: &'static str,
		/// The description of the metric.
		pub description: &'static str,
	}

	/// `CounterVec` metric definition.
	pub struct CounterVecDefinition<'a> {
		/// The name of the metric.
		pub name: &'static str,
		/// The description of the metric.
		pub description: &'static str,
		/// The label names of the metric.
		pub labels: &'a [&'static str],
	}

	/// `Histogram` metric definition
	pub struct HistogramDefinition<'a> {
		/// The name of the metric.
		pub name: &'static str,
		/// The description of the metric.
		pub description: &'static str,
		/// The buckets for the histogram
		pub buckets: &'a [f64],
	}

	/// Counts parachain inherent data weights. Use `before` and `after` labels to differentiate
	/// between the weight before and after filtering.
	pub const PARACHAIN_INHERENT_DATA_WEIGHT: CounterVecDefinition = CounterVecDefinition {
		name: "polkadot_parachain_inherent_data_weight",
		description: "Inherent data weight before and after filtering",
		labels: &["when"],
	};

	/// Counts the number of bitfields processed in `process_inherent_data`.
	pub const PARACHAIN_INHERENT_DATA_BITFIELDS_PROCESSED: CounterDefinition = CounterDefinition {
		name: "polkadot_parachain_inherent_data_bitfields_processed",
		description: "Counts the number of bitfields processed in `process_inherent_data`.",
	};

	/// Counts the `total`, `sanitized` and `included` number of parachain block candidates
	/// in `process_inherent_data`.
	pub const PARACHAIN_INHERENT_DATA_CANDIDATES_PROCESSED: CounterVecDefinition =
		CounterVecDefinition {
			name: "polkadot_parachain_inherent_data_candidates_processed",
			description:
				"Counts the number of parachain block candidates processed in `process_inherent_data`.",
			labels: &["category"],
		};

	/// Counts the number of `imported`, `current` and `concluded_invalid` dispute statements sets
	/// processed in `process_inherent_data`. The `current` label refers to the disputes statement
	/// sets of the current session.
	pub const PARACHAIN_INHERENT_DATA_DISPUTE_SETS_PROCESSED: CounterVecDefinition =
		CounterVecDefinition {
			name: "polkadot_parachain_inherent_data_dispute_sets_processed",
			description:
				"Counts the number of dispute statements sets processed in `process_inherent_data`.",
			labels: &["category"],
		};

	/// Counts the number of `valid` and `invalid` bitfields signature checked in
	/// `process_inherent_data`.
	pub const PARACHAIN_CREATE_INHERENT_BITFIELDS_SIGNATURE_CHECKS: CounterVecDefinition =
		CounterVecDefinition {
			name: "polkadot_parachain_create_inherent_bitfields_signature_checks",
			description:
				"Counts the number of bitfields signature checked in `process_inherent_data`.",
			labels: &["validity"],
		};

	/// Measures how much time does it take to verify a single validator signature of a dispute
	/// statement
	pub const PARACHAIN_VERIFY_DISPUTE_SIGNATURE: HistogramDefinition =
		HistogramDefinition {
			name: "polkadot_parachain_verify_dispute_signature",
			description: "How much time does it take to verify a single validator signature of a dispute statement, in seconds",
			buckets: &[0.0, 0.00005, 0.00006, 0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.3, 0.5, 1.0],
	};
}
