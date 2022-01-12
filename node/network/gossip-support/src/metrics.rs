// Copyright 2021 Parity Technologies (UK) Ltd.
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

use polkadot_node_subsystem_util::{
	metrics,
	metrics::{
		prometheus,
		prometheus::{Gauge, PrometheusError, Registry, U64},
	},
};

/// Dispute Distribution metrics.
#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Clone)]
struct MetricsInner {
	/// Tracks authority status.
	is_authority: Gauge<U64>,
}

impl Metrics {
	/// Set the authority flag.
	pub fn on_is_authority(&self) {
		if let Some(metrics) = &self.0 {
			metrics.is_authority.set(1);
		}
	}

	/// Unset the authority flag.
	pub fn on_is_not_authority(&self) {
		if let Some(metrics) = &self.0 {
			metrics.is_authority.set(0);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MetricsInner {
			is_authority: prometheus::register(
				Gauge::new("polkadot_node_is_authority", "Tracks the node authority status.")?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
