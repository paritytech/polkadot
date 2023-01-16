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
	/// Tracks authority status for producing relay chain blocks.
	is_authority: Gauge<U64>,
	/// Tracks authority status for parachain approval checking.
	is_parachain_validator: Gauge<U64>,
}

impl Metrics {
	/// Dummy constructor for testing.
	#[cfg(test)]
	pub fn new_dummy() -> Self {
		Self(None)
	}

	/// Set the `relaychain validator` metric.
	pub fn on_is_authority(&self) {
		if let Some(metrics) = &self.0 {
			metrics.is_authority.set(1);
		}
	}

	/// Unset the `relaychain validator` metric.
	pub fn on_is_not_authority(&self) {
		if let Some(metrics) = &self.0 {
			metrics.is_authority.set(0);
		}
	}

	/// Set the `parachain validator` metric.
	pub fn on_is_parachain_validator(&self) {
		if let Some(metrics) = &self.0 {
			metrics.is_parachain_validator.set(1);
		}
	}

	/// Unset the `parachain validator` metric.
	pub fn on_is_not_parachain_validator(&self) {
		if let Some(metrics) = &self.0 {
			metrics.is_parachain_validator.set(0);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MetricsInner {
			is_authority: prometheus::register(
				Gauge::new("polkadot_node_is_active_validator", "Tracks if the validator is in the active set. \
				Updates at session boundary.")?,
				registry,
			)?,
			is_parachain_validator: prometheus::register(
				Gauge::new("polkadot_node_is_parachain_validator", 
				"Tracks if the validator participates in parachain consensus. Parachain validators are a \
				subset of the active set validators that perform approval checking of all parachain candidates in a session.\
				Updates at session boundary.")?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
