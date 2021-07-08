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

//! Prometheus metrics related to the overseer and its channels.

use super::*;
use polkadot_node_metrics::metrics::{self, prometheus};

/// Overseer Prometheus metrics.
#[derive(Clone)]
struct MetricsInner {
	activated_heads_total: prometheus::Counter<prometheus::U64>,
	deactivated_heads_total: prometheus::Counter<prometheus::U64>,
	messages_relayed_total: prometheus::Counter<prometheus::U64>,
	to_subsystem_bounded_sent: prometheus::GaugeVec<prometheus::U64>,
	to_subsystem_bounded_received: prometheus::GaugeVec<prometheus::U64>,
	to_subsystem_unbounded_sent: prometheus::GaugeVec<prometheus::U64>,
	to_subsystem_unbounded_received: prometheus::GaugeVec<prometheus::U64>,
	signals_sent: prometheus::GaugeVec<prometheus::U64>,
	signals_received: prometheus::GaugeVec<prometheus::U64>,
}


/// A sharable metrics type for usage with the overseer.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub(crate) fn on_head_activated(&self) {
		if let Some(metrics) = &self.0 {
			metrics.activated_heads_total.inc();
		}
	}

	pub(crate) fn on_head_deactivated(&self) {
		if let Some(metrics) = &self.0 {
			metrics.deactivated_heads_total.inc();
		}
	}

	pub(crate) fn on_message_relayed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.messages_relayed_total.inc();
		}
	}

	pub(crate) fn channel_fill_level_snapshot(
		&self,
		collection: impl IntoIterator<Item=(&'static str, SubsystemMeterReadouts)>,
	) {
		if let Some(metrics) = &self.0 {
			collection.into_iter().for_each(
					|(name, readouts): (_, SubsystemMeterReadouts)| {
						metrics.to_subsystem_bounded_sent.with_label_values(&[name])
							.set(readouts.bounded.sent as u64);

						metrics.to_subsystem_bounded_received.with_label_values(&[name])
							.set(readouts.bounded.received as u64);

						metrics.to_subsystem_unbounded_sent.with_label_values(&[name])
							.set(readouts.unbounded.sent as u64);

						metrics.to_subsystem_unbounded_received.with_label_values(&[name])
							.set(readouts.unbounded.received as u64);

						metrics.signals_sent.with_label_values(&[name])
							.set(readouts.signals.sent as u64);

						metrics.signals_received.with_label_values(&[name])
							.set(readouts.signals.received as u64);
					}
			);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			activated_heads_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_activated_heads_total",
					"Number of activated heads."
				)?,
				registry,
			)?,
			deactivated_heads_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_deactivated_heads_total",
					"Number of deactivated heads."
				)?,
				registry,
			)?,
			messages_relayed_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_messages_relayed_total",
					"Number of messages relayed by Overseer."
				)?,
				registry,
			)?,
			to_subsystem_bounded_sent: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_bounded_sent",
						"Number of elements sent to subsystems' bounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			to_subsystem_bounded_received: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_bounded_received",
						"Number of elements received by subsystems' bounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			to_subsystem_unbounded_sent: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_unbounded_sent",
						"Number of elements sent to subsystems' unbounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			to_subsystem_unbounded_received: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_unbounded_received",
						"Number of elements received by subsystems' unbounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			signals_sent: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_overseer_signals_sent",
						"Number of signals sent by overseer to subsystems",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			signals_received: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_overseer_signals_received",
						"Number of signals received by subsystems from overseer",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

impl fmt::Debug for Metrics {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("Metrics {{...}}")
	}
}
