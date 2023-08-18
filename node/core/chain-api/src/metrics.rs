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

use polkadot_node_metrics::metrics::{self, prometheus};

#[derive(Clone)]
pub(crate) struct MetricsInner {
	pub(crate) chain_api_requests: prometheus::CounterVec<prometheus::U64>,
	pub(crate) block_number: prometheus::Histogram,
	pub(crate) block_header: prometheus::Histogram,
	pub(crate) block_weight: prometheus::Histogram,
	pub(crate) finalized_block_hash: prometheus::Histogram,
	pub(crate) finalized_block_number: prometheus::Histogram,
	pub(crate) ancestors: prometheus::Histogram,
}

/// Chain API metrics.
#[derive(Default, Clone)]
pub struct Metrics(pub(crate) Option<MetricsInner>);

impl Metrics {
	pub fn on_request(&self, succeeded: bool) {
		if let Some(metrics) = &self.0 {
			if succeeded {
				metrics.chain_api_requests.with_label_values(&["succeeded"]).inc();
			} else {
				metrics.chain_api_requests.with_label_values(&["failed"]).inc();
			}
		}
	}

	/// Provide a timer for `block_number` which observes on drop.
	pub fn time_block_number(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_number.start_timer())
	}

	/// Provide a timer for `block_header` which observes on drop.
	pub fn time_block_header(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_header.start_timer())
	}

	/// Provide a timer for `block_weight` which observes on drop.
	pub fn time_block_weight(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_weight.start_timer())
	}

	/// Provide a timer for `finalized_block_hash` which observes on drop.
	pub fn time_finalized_block_hash(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.finalized_block_hash.start_timer())
	}

	/// Provide a timer for `finalized_block_number` which observes on drop.
	pub fn time_finalized_block_number(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.finalized_block_number.start_timer())
	}

	/// Provide a timer for `ancestors` which observes on drop.
	pub fn time_ancestors(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.ancestors.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			chain_api_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_chain_api_requests_total",
						"Number of Chain API requests served.",
					),
					&["success"],
				)?,
				registry,
			)?,
			block_number: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_chain_api_block_number",
					"Time spent within `chain_api::block_number`",
				))?,
				registry,
			)?,
			block_header: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_chain_api_block_headers",
					"Time spent within `chain_api::block_headers`",
				))?,
				registry,
			)?,
			block_weight: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_chain_api_block_weight",
					"Time spent within `chain_api::block_weight`",
				))?,
				registry,
			)?,
			finalized_block_hash: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_chain_api_finalized_block_hash",
					"Time spent within `chain_api::finalized_block_hash`",
				))?,
				registry,
			)?,
			finalized_block_number: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_chain_api_finalized_block_number",
					"Time spent within `chain_api::finalized_block_number`",
				))?,
				registry,
			)?,
			ancestors: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_chain_api_ancestors",
					"Time spent within `chain_api::ancestors`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
