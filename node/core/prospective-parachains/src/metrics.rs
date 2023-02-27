// Copyright 2023 Parity Technologies (UK) Ltd.
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
	pub(crate) prune_view_candidate_storage: prometheus::Histogram,
}

/// Candidate backing metrics.
#[derive(Default, Clone)]
pub struct Metrics(pub(crate) Option<MetricsInner>);

impl Metrics {
	/// Provide a timer for handling `prune_view_candidate_storage` which observes on drop.
	pub fn time_prune_view_candidate_storage(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.prune_view_candidate_storage.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			prune_view_candidate_storage: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_prospective_parachains_prune_view_candidate_storage",
					"Time spent within `prospective_parachains::prune_view_candidate_storage`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
