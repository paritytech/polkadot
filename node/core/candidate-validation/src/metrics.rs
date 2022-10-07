// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

use super::{ValidationFailed, ValidationResult};
use polkadot_node_subsystem_util::metrics::{self, prometheus};

#[derive(Clone)]
pub(crate) struct MetricsInner {
	pub(crate) validation_requests: prometheus::CounterVec<prometheus::U64>,
	pub(crate) validate_from_chain_state: prometheus::Histogram,
	pub(crate) validate_from_exhaustive: prometheus::Histogram,
	pub(crate) validate_candidate_exhaustive: prometheus::Histogram,
}

/// Candidate validation metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub fn on_validation_event(&self, event: &Result<ValidationResult, ValidationFailed>) {
		if let Some(metrics) = &self.0 {
			match event {
				Ok(ValidationResult::Valid(_, _)) => {
					metrics.validation_requests.with_label_values(&["valid"]).inc();
				},
				Ok(ValidationResult::Invalid(_)) => {
					metrics.validation_requests.with_label_values(&["invalid"]).inc();
				},
				Err(_) => {
					metrics.validation_requests.with_label_values(&["validation failure"]).inc();
				},
			}
		}
	}

	/// Provide a timer for `validate_from_chain_state` which observes on drop.
	pub fn time_validate_from_chain_state(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.validate_from_chain_state.start_timer())
	}

	/// Provide a timer for `validate_from_exhaustive` which observes on drop.
	pub fn time_validate_from_exhaustive(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.validate_from_exhaustive.start_timer())
	}

	/// Provide a timer for `validate_candidate_exhaustive` which observes on drop.
	pub fn time_validate_candidate_exhaustive(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.validate_candidate_exhaustive.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			validation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_validation_requests_total",
						"Number of validation requests served.",
					),
					&["validity"],
				)?,
				registry,
			)?,
			validate_from_chain_state: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_candidate_validation_validate_from_chain_state",
					"Time spent within `candidate_validation::validate_from_chain_state`",
				))?,
				registry,
			)?,
			validate_from_exhaustive: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_candidate_validation_validate_from_exhaustive",
					"Time spent within `candidate_validation::validate_from_exhaustive`",
				))?,
				registry,
			)?,
			validate_candidate_exhaustive: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_candidate_validation_validate_candidate_exhaustive",
					"Time spent within `candidate_validation::validate_candidate_exhaustive`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
