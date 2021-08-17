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

#[derive(Clone)]
struct MetricsInner {
	votes: prometheus::Histogram,
	concluded: prometheus::Histogram,
}

/// Candidate validation metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_vote_valid(&self) {
		if let Some(metrics) = self.0 {
			metrics.vote.with_label_values(&["valid"]).inc();
		}
	}

	fn on_vote_invalid(&self) {
		if let Some(metrics) = self.0 {
			metrics.vote.with_label_values(&["invalid"]).inc();
		}
	}

	fn on_concluded_invalid(&self) {
		if let Some(metrics) = self.0 {
			metrics.concluded.with_label_values(&["valid"]).inc();
		}
	}

	fn on_concluded_invalid(&self) {
		if let Some(metrics) = self.0 {
			metrics.concluded.with_label_values(&["invalid"]).inc();
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			validation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_validation_requests_total",
						"Number of validation requests served.",
					),
					&["validity"],
				)?,
				registry,
			)?,
			validate_from_chain_state: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_candidate_validation_validate_from_chain_state",
					"Time spent within `candidate_validation::validate_from_chain_state`",
				))?,
				registry,
			)?,
			validate_from_exhaustive: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_candidate_validation_validate_from_exhaustive",
					"Time spent within `candidate_validation::validate_from_exhaustive`",
				))?,
				registry,
			)?,
			validate_candidate_exhaustive: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_candidate_validation_validate_candidate_exhaustive",
					"Time spent within `candidate_validation::validate_candidate_exhaustive`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
