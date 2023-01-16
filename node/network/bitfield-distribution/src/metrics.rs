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

#[derive(Clone)]
struct MetricsInner {
	sent_own_availability_bitfields: prometheus::Counter<prometheus::U64>,
	received_availability_bitfields: prometheus::Counter<prometheus::U64>,
	active_leaves_update: prometheus::Histogram,
	handle_bitfield_distribution: prometheus::Histogram,
	handle_network_msg: prometheus::Histogram,
}

/// Bitfield Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub(crate) fn on_own_bitfield_sent(&self) {
		if let Some(metrics) = &self.0 {
			metrics.sent_own_availability_bitfields.inc();
		}
	}

	pub(crate) fn on_bitfield_received(&self) {
		if let Some(metrics) = &self.0 {
			metrics.received_availability_bitfields.inc();
		}
	}

	/// Provide a timer for `active_leaves_update` which observes on drop.
	pub(crate) fn time_active_leaves_update(
		&self,
	) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.active_leaves_update.start_timer())
	}

	/// Provide a timer for `handle_bitfield_distribution` which observes on drop.
	pub(crate) fn time_handle_bitfield_distribution(
		&self,
	) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.handle_bitfield_distribution.start_timer())
	}

	/// Provide a timer for `handle_network_msg` which observes on drop.
	pub(crate) fn time_handle_network_msg(&self) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.handle_network_msg.start_timer())
	}
}

impl MetricsTrait for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			sent_own_availability_bitfields: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_sent_own_availabilty_bitfields_total",
					"Number of own availability bitfields sent to other peers.",
				)?,
				registry,
			)?,
			received_availability_bitfields: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_received_availabilty_bitfields_total",
					"Number of valid availability bitfields received from other peers.",
				)?,
				registry,
			)?,
			active_leaves_update: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_bitfield_distribution_active_leaves_update",
					"Time spent within `bitfield_distribution::active_leaves_update`",
				))?,
				registry,
			)?,
			handle_bitfield_distribution: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_bitfield_distribution_handle_bitfield_distribution",
					"Time spent within `bitfield_distribution::handle_bitfield_distribution`",
				))?,
				registry,
			)?,
			handle_network_msg: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_bitfield_distribution_handle_network_msg",
					"Time spent within `bitfield_distribution::handle_network_msg`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
