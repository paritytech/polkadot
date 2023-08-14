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

use crate::disputes::prioritized_selection::PartitionedDisputes;
use polkadot_node_subsystem_util::metrics::{self, prometheus};

#[derive(Clone)]
struct MetricsInner {
	/// Tracks successful/unsuccessful inherent data requests
	inherent_data_requests: prometheus::CounterVec<prometheus::U64>,
	/// How much time the `RequestInherentData` processing takes
	request_inherent_data_duration: prometheus::Histogram,
	/// How much time `ProvisionableData` processing takes
	provisionable_data_duration: prometheus::Histogram,
	/// Bitfields array length in `ProvisionerInherentData` (the result for `RequestInherentData`)
	inherent_data_response_bitfields: prometheus::Histogram,

	/// The following metrics track how many disputes/votes the runtime will have to process. These
	/// will count all recent statements meaning every dispute from last sessions: 10 min on
	/// Rococo, 60 min on Kusama and 4 hours on Polkadot. The metrics are updated only when the
	/// node authors a block, so values vary across nodes.
	inherent_data_dispute_statement_sets: prometheus::Counter<prometheus::U64>,
	inherent_data_dispute_statements: prometheus::CounterVec<prometheus::U64>,

	/// The disputes received from `disputes-coordinator` by partition
	partitioned_disputes: prometheus::CounterVec<prometheus::U64>,

	/// The disputes fetched from the runtime.
	fetched_onchain_disputes: prometheus::Counter<prometheus::U64>,
}

/// Provisioner metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	/// Creates new dummy `Metrics` instance. Used for testing only.
	#[cfg(test)]
	pub fn new_dummy() -> Metrics {
		Metrics(None)
	}

	pub(crate) fn on_inherent_data_request(&self, response: Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			match response {
				Ok(()) => metrics.inherent_data_requests.with_label_values(&["succeeded"]).inc(),
				Err(()) => metrics.inherent_data_requests.with_label_values(&["failed"]).inc(),
			}
		}
	}

	/// Provide a timer for `request_inherent_data` which observes on drop.
	pub(crate) fn time_request_inherent_data(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.request_inherent_data_duration.start_timer())
	}

	/// Provide a timer for `provisionable_data` which observes on drop.
	pub(crate) fn time_provisionable_data(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.provisionable_data_duration.start_timer())
	}

	pub(crate) fn observe_inherent_data_bitfields_count(&self, bitfields_count: usize) {
		self.0.as_ref().map(|metrics| {
			metrics.inherent_data_response_bitfields.observe(bitfields_count as f64)
		});
	}

	pub(crate) fn inc_valid_statements_by(&self, votes: usize) {
		if let Some(metrics) = &self.0 {
			metrics
				.inherent_data_dispute_statements
				.with_label_values(&["valid"])
				.inc_by(votes.try_into().unwrap_or(0));
		}
	}

	pub(crate) fn inc_invalid_statements_by(&self, votes: usize) {
		if let Some(metrics) = &self.0 {
			metrics
				.inherent_data_dispute_statements
				.with_label_values(&["invalid"])
				.inc_by(votes.try_into().unwrap_or(0));
		}
	}

	pub(crate) fn inc_dispute_statement_sets_by(&self, disputes: usize) {
		if let Some(metrics) = &self.0 {
			metrics
				.inherent_data_dispute_statement_sets
				.inc_by(disputes.try_into().unwrap_or(0));
		}
	}

	pub(crate) fn on_partition_recent_disputes(&self, disputes: &PartitionedDisputes) {
		if let Some(metrics) = &self.0 {
			let PartitionedDisputes {
				inactive_unknown_onchain,
				inactive_unconcluded_onchain: inactive_unconcluded_known_onchain,
				active_unknown_onchain,
				active_unconcluded_onchain,
				active_concluded_onchain,
				inactive_concluded_onchain: inactive_concluded_known_onchain,
			} = disputes;

			metrics
				.partitioned_disputes
				.with_label_values(&["inactive_unknown_onchain"])
				.inc_by(inactive_unknown_onchain.len().try_into().unwrap_or(0));
			metrics
				.partitioned_disputes
				.with_label_values(&["inactive_unconcluded_known_onchain"])
				.inc_by(inactive_unconcluded_known_onchain.len().try_into().unwrap_or(0));
			metrics
				.partitioned_disputes
				.with_label_values(&["active_unknown_onchain"])
				.inc_by(active_unknown_onchain.len().try_into().unwrap_or(0));
			metrics
				.partitioned_disputes
				.with_label_values(&["active_unconcluded_onchain"])
				.inc_by(active_unconcluded_onchain.len().try_into().unwrap_or(0));
			metrics
				.partitioned_disputes
				.with_label_values(&["active_concluded_onchain"])
				.inc_by(active_concluded_onchain.len().try_into().unwrap_or(0));
			metrics
				.partitioned_disputes
				.with_label_values(&["inactive_concluded_known_onchain"])
				.inc_by(inactive_concluded_known_onchain.len().try_into().unwrap_or(0));
		}
	}

	pub(crate) fn on_fetched_onchain_disputes(&self, onchain_count: u64) {
		if let Some(metrics) = &self.0 {
			metrics.fetched_onchain_disputes.inc_by(onchain_count);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			inherent_data_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_inherent_data_requests_total",
						"Number of InherentData requests served by provisioner.",
					),
					&["success"],
				)?,
				registry,
			)?,
			request_inherent_data_duration: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_provisioner_request_inherent_data_time",
					"Time spent within `provisioner::request_inherent_data`",
				))?,
				registry,
			)?,
			provisionable_data_duration: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_provisioner_provisionable_data_time",
					"Time spent within `provisioner::provisionable_data`",
				))?,
				registry,
			)?,
			inherent_data_dispute_statements: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_inherent_data_dispute_statements",
						"Number of dispute statements passed to `create_inherent()`.",
					),
					&["validity"],
				)?,
				&registry,
			)?,
			inherent_data_dispute_statement_sets: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_inherent_data_dispute_statement_sets",
					"Number of dispute statements sets passed to `create_inherent()`.",
				)?,
				registry,
			)?,
			inherent_data_response_bitfields: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_provisioner_inherent_data_response_bitfields_sent",
						"Number of inherent bitfields sent in response to `ProvisionerMessage::RequestInherentData`.",
					).buckets(vec![0.0, 25.0, 50.0, 100.0, 150.0, 200.0, 250.0, 300.0, 400.0, 500.0, 600.0]),
				)?,
				registry,
			)?,
			partitioned_disputes: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_provisioner_partitioned_disputes",
						"Number of disputes partitioned by type.",
					),
					&["partition"],
				)?,
				&registry,
			)?,
			fetched_onchain_disputes: prometheus::register(
				prometheus::Counter::new("polkadot_parachain_fetched_onchain_disputes", "Number of disputes fetched from the runtime"
				)?,
				&registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
