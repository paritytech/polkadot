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
pub(crate) struct MetricsInner {
	received_availability_chunks_total: prometheus::Counter<prometheus::U64>,
	pruning: prometheus::Histogram,
	process_block_finalized: prometheus::Histogram,
	block_activated: prometheus::Histogram,
	process_message: prometheus::Histogram,
	store_available_data: prometheus::Histogram,
	store_chunk: prometheus::Histogram,
	get_chunk: prometheus::Histogram,
}

/// Availability metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub(crate) fn on_chunks_received(&self, count: usize) {
		if let Some(metrics) = &self.0 {
			// assume usize fits into u64
			let by = u64::try_from(count).unwrap_or_default();
			metrics.received_availability_chunks_total.inc_by(by);
		}
	}

	/// Provide a timer for `prune_povs` which observes on drop.
	pub(crate) fn time_pruning(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.pruning.start_timer())
	}

	/// Provide a timer for `process_block_finalized` which observes on drop.
	pub(crate) fn time_process_block_finalized(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_block_finalized.start_timer())
	}

	/// Provide a timer for `block_activated` which observes on drop.
	pub(crate) fn time_block_activated(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_activated.start_timer())
	}

	/// Provide a timer for `process_message` which observes on drop.
	pub(crate) fn time_process_message(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_message.start_timer())
	}

	/// Provide a timer for `store_available_data` which observes on drop.
	pub(crate) fn time_store_available_data(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.store_available_data.start_timer())
	}

	/// Provide a timer for `store_chunk` which observes on drop.
	pub(crate) fn time_store_chunk(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.store_chunk.start_timer())
	}

	/// Provide a timer for `get_chunk` which observes on drop.
	pub(crate) fn time_get_chunk(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.get_chunk.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			received_availability_chunks_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_received_availability_chunks_total",
					"Number of availability chunks received.",
				)?,
				registry,
			)?,
			pruning: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_av_store_pruning",
					"Time spent within `av_store::prune_all`",
				))?,
				registry,
			)?,
			process_block_finalized: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_av_store_process_block_finalized",
					"Time spent within `av_store::process_block_finalized`",
				))?,
				registry,
			)?,
			block_activated: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_av_store_block_activated",
					"Time spent within `av_store::process_block_activated`",
				))?,
				registry,
			)?,
			process_message: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_av_store_process_message",
					"Time spent within `av_store::process_message`",
				))?,
				registry,
			)?,
			store_available_data: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_av_store_store_available_data",
					"Time spent within `av_store::store_available_data`",
				))?,
				registry,
			)?,
			store_chunk: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_av_store_store_chunk",
					"Time spent within `av_store::store_chunk`",
				))?,
				registry,
			)?,
			get_chunk: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_av_store_get_chunk",
						"Time spent fetching requested chunks.`",
					)
					.buckets(vec![
						0.000625, 0.00125, 0.0025, 0.005, 0.0075, 0.01, 0.025, 0.05, 0.1, 0.25,
						0.5, 1.0, 2.5, 5.0, 10.0,
					]),
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
