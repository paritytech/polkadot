// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Metrics helpers
//!
//! Collects a bunch of metrics providers and related features such as
//! `Metronome` for usage with metrics collections.
//!
//! This crate also reexports Prometheus metric types which are expected to be implemented by subsystems.

#![deny(missing_docs)]
#![deny(unused_imports)]

pub use metered;

/// Cyclic metric collection support.
pub mod metronome;
pub use self::metronome::Metronome;

#[cfg(feature = "runtime-metrics")]
pub mod runtime;
#[cfg(feature = "runtime-metrics")]
pub use self::runtime::logger_hook;

/// Export a dummy logger hook when the `runtime-metrics` feature is not enabled.
#[cfg(not(feature = "runtime-metrics"))]
pub fn logger_hook() -> impl FnOnce(&mut sc_cli::LoggerBuilder, &sc_service::Configuration) {
	|_logger_builder, _config| {}
}

/// This module reexports Prometheus types and defines the [`Metrics`](metrics::Metrics) trait.
pub mod metrics {
	/// Reexport Substrate Prometheus types.
	pub use substrate_prometheus_endpoint as prometheus;

	/// Subsystem- or job-specific Prometheus metrics.
	///
	/// Usually implemented as a wrapper for `Option<ActualMetrics>`
	/// to ensure `Default` bounds or as a dummy type ().
	/// Prometheus metrics internally hold an `Arc` reference, so cloning them is fine.
	pub trait Metrics: Default + Clone {
		/// Try to register metrics in the Prometheus registry.
		fn try_register(
			registry: &prometheus::Registry,
		) -> Result<Self, prometheus::PrometheusError>;

		/// Convenience method to register metrics in the optional Prometheus registry.
		///
		/// If no registry is provided, returns `Default::default()`. Otherwise, returns the same
		/// thing that `try_register` does.
		fn register(
			registry: Option<&prometheus::Registry>,
		) -> Result<Self, prometheus::PrometheusError> {
			match registry {
				None => Ok(Self::default()),
				Some(registry) => Self::try_register(registry),
			}
		}
	}

	// dummy impl
	impl Metrics for () {
		fn try_register(
			_registry: &prometheus::Registry,
		) -> Result<(), prometheus::PrometheusError> {
			Ok(())
		}
	}
}

#[cfg(all(feature = "runtime-metrics", not(feature = "runtime-benchmarks"), test))]
mod tests;
