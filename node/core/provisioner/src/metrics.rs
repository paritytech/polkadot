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

use polkadot_node_subsystem_util::metrics::{self, prometheus};

#[derive(Clone)]
struct MetricsInner {
	inherent_data_requests: prometheus::CounterVec<prometheus::U64>,
	request_inherent_data: prometheus::Histogram,
	provisionable_data: prometheus::Histogram,
}

/// Provisioner metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
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
		self.0.as_ref().map(|metrics| metrics.request_inherent_data.start_timer())
	}

	/// Provide a timer for `provisionable_data` which observes on drop.
	pub(crate) fn time_provisionable_data(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.provisionable_data.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			inherent_data_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_inherent_data_requests_total",
						"Number of InherentData requests served by provisioner.",
					),
					&["success"],
				)?,
				registry,
			)?,
			request_inherent_data: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_provisioner_request_inherent_data_time",
					"Time spent within `provisioner::request_inherent_data`",
				))?,
				registry,
			)?,
			provisionable_data: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_provisioner_provisionable_data_time",
					"Time spent within `provisioner::provisionable_data`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
