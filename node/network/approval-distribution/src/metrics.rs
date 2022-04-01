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

use polkadot_node_subsystem_util::metrics::{prometheus, Metrics as MetricsTrait};
use super::SentMessagesStats;

/// Approval Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Clone)]
struct MetricsInner {
	assignments_imported_total: prometheus::Counter<prometheus::U64>,
	approvals_imported_total: prometheus::Counter<prometheus::U64>,
	unified_with_peer_total: prometheus::Counter<prometheus::U64>,
	aggression_l1_messages_total: prometheus::Counter<prometheus::U64>,
	aggression_l2_messages_total: prometheus::Counter<prometheus::U64>,

	time_unify_with_peer: prometheus::Histogram,
	time_import_pending_now_known: prometheus::Histogram,
	time_awaiting_approval_voting: prometheus::Histogram,

	// TODO [now]: these metrics are (for the most part) temporary for figuring
	// out why there are so many messages.
	basic_circulation_messages_total: prometheus::CounterVec<prometheus::U64>,
	basic_circulation_packets_total: prometheus::CounterVec<prometheus::U64>,
	unify_with_peer_messages_total: prometheus::CounterVec<prometheus::U64>,
	unify_with_peer_packets_total: prometheus::CounterVec<prometheus::U64>,
	resend_messages_total: prometheus::CounterVec<prometheus::U64>,
	resend_packets_total: prometheus::CounterVec<prometheus::U64>,
	aggression_messages_total: prometheus::CounterVec<prometheus::U64>,
	aggression_packets_total: prometheus::CounterVec<prometheus::U64>,
	new_topology_messages_total: prometheus::CounterVec<prometheus::U64>,
	new_topology_packets_total: prometheus::CounterVec<prometheus::U64>,
}

impl Metrics {
	pub(crate) fn on_assignment_imported(&self) {
		if let Some(metrics) = &self.0 {
			metrics.assignments_imported_total.inc();
		}
	}

	pub(crate) fn on_approval_imported(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_imported_total.inc();
		}
	}

	pub(crate) fn on_unify_with_peer(&self) {
		if let Some(metrics) = &self.0 {
			metrics.unified_with_peer_total.inc();
		}
	}

	pub(crate) fn time_unify_with_peer(&self) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_unify_with_peer.start_timer())
	}

	pub(crate) fn time_import_pending_now_known(
		&self,
	) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.time_import_pending_now_known.start_timer())
	}

	pub(crate) fn time_awaiting_approval_voting(
		&self,
	) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.time_awaiting_approval_voting.start_timer())
	}

	pub(crate) fn on_aggression_l1(&self) {
		if let Some(metrics) = &self.0 {
			metrics.aggression_l1_messages_total.inc();
		}
	}

	pub(crate) fn on_aggression_l2(&self) {
		if let Some(metrics) = &self.0 {
			metrics.aggression_l2_messages_total.inc();
		}
	}

	pub(crate) fn note_basic_circulation_stats(&self, stats: SentMessagesStats) {
		if let Some(metrics) = &self.0 {
			note_sent_message_stats(
				stats,
				&metrics.basic_circulation_messages_total,
				&metrics.basic_circulation_packets_total,
			)
		}
	}

	pub(crate) fn note_unify_with_peer_stats(&self, stats: SentMessagesStats) {
		if let Some(metrics) = &self.0 {
			note_sent_message_stats(
				stats,
				&metrics.unify_with_peer_messages_total,
				&metrics.unify_with_peer_packets_total,
			)
		}
	}

	pub(crate) fn note_resend_stats(&self, stats: SentMessagesStats) {
		if let Some(metrics) = &self.0 {
			note_sent_message_stats(
				stats,
				&metrics.resend_messages_total,
				&metrics.resend_packets_total,
			)
		}
	}

	pub(crate) fn note_aggression_stats(&self, stats: SentMessagesStats) {
		if let Some(metrics) = &self.0 {
			note_sent_message_stats(
				stats,
				&metrics.aggression_messages_total,
				&metrics.aggression_packets_total,
			)
		}
	}

	pub(crate) fn note_new_topology_stats(&self, stats: SentMessagesStats) {
		if let Some(metrics) = &self.0 {
			note_sent_message_stats(
				stats,
				&metrics.new_topology_messages_total,
				&metrics.new_topology_packets_total,
			)
		}
	}
}

fn note_sent_message_stats(
	stats: SentMessagesStats,
	message_counters: &prometheus::CounterVec<prometheus::U64>,
	packet_counters: &prometheus::CounterVec<prometheus::U64>,
) {
	message_counters.with_label_values(&["assignments"]).inc_by(stats.assignments as u64);
	message_counters.with_label_values(&["approvals"]).inc_by(stats.approvals as u64);

	packet_counters.with_label_values(&["assignments"]).inc_by(stats.assignment_packets as u64);
	packet_counters.with_label_values(&["approvals"]).inc_by(stats.approval_packets as u64);
}

impl MetricsTrait for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			assignments_imported_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_assignments_imported_total",
					"Number of valid assignments imported locally or from other peers.",
				)?,
				registry,
			)?,
			approvals_imported_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_approvals_imported_total",
					"Number of valid approvals imported locally or from other peers.",
				)?,
				registry,
			)?,
			unified_with_peer_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_unified_with_peer_total",
					"Number of times `unify_with_peer` is called.",
				)?,
				registry,
			)?,
			aggression_l1_messages_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_approval_distribution_aggression_l1_messages_total",
					"Number of messages in approval distribution for which aggression L1 has been triggered",
				)?,
				registry,
			)?,
			aggression_l2_messages_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_approval_distribution_aggression_l2_messages_total",
					"Number of messages in approval distribution for which aggression L2 has been triggered",
				)?,
				registry,
			)?,
			time_unify_with_peer: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_time_unify_with_peer",
					"Time spent within fn `unify_with_peer`.",
				))?,
				registry,
			)?,
			time_import_pending_now_known: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_time_import_pending_now_known",
					"Time spent on importing pending assignments and approvals.",
				))?,
				registry,
			)?,
			time_awaiting_approval_voting: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_time_awaiting_approval_voting",
					"Time spent awaiting a reply from the Approval Voting Subsystem.",
				))?,
				registry,
			)?,
			basic_circulation_messages_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_basic_circulation_messages_total",
						"Number of assignments and approvals sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			basic_circulation_packets_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_basic_circulation_packets_total",
						"Number of packets sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			unify_with_peer_messages_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_unify_with_peer_messages_total",
						"Number of assignments and approvals sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			unify_with_peer_packets_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_unify_with_peer_packets_total",
						"Number of packets sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			resend_messages_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_resend_messages_total",
						"Number of assignments and approvals sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			resend_packets_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_resend_packets_total",
						"Number of packets sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			aggression_messages_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_aggression_messages_total",
						"Number of assignments and approvals sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			aggression_packets_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_aggression_packets_total",
						"Number of packets sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			new_topology_messages_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_new_topology_messages_total",
						"Number of assignments and approvals sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
			new_topology_packets_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approval_new_topology_packets_total",
						"Number of packets sent by basic circulation",
					),
					&["kind"]
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
