// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

pub use float_json_value::FloatJsonValueMetric;
pub use global::GlobalMetrics;
pub use substrate_prometheus_endpoint::{
	prometheus::core::{Atomic, Collector},
	register, Counter, CounterVec, Gauge, GaugeVec, Opts, PrometheusError, Registry, F64, I64, U64,
};

use async_std::sync::{Arc, RwLock};
use async_trait::async_trait;
use std::{fmt::Debug, time::Duration};

mod float_json_value;
mod global;

/// Shared reference to `f64` value that is updated by the metric.
pub type F64SharedRef = Arc<RwLock<Option<f64>>>;
/// Int gauge metric type.
pub type IntGauge = Gauge<U64>;

/// Unparsed address that needs to be used to expose Prometheus metrics.
#[derive(Debug, Clone)]
pub struct MetricsAddress {
	/// Serve HTTP requests at given host.
	pub host: String,
	/// Serve HTTP requests at given port.
	pub port: u16,
}

/// Prometheus endpoint MetricsParams.
#[derive(Debug, Clone)]
pub struct MetricsParams {
	/// Interface and TCP port to be used when exposing Prometheus metrics.
	pub address: Option<MetricsAddress>,
	/// Metrics registry. May be `Some(_)` if several components share the same endpoint.
	pub registry: Registry,
}

/// Metric API.
pub trait Metric: Clone + Send + Sync + 'static {
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError>;
}

/// Standalone metric API.
///
/// Metrics of this kind know how to update themselves, so we may just spawn and forget the
/// asynchronous self-update task.
#[async_trait]
pub trait StandaloneMetric: Metric {
	/// Update metric values.
	async fn update(&self);

	/// Metrics update interval.
	fn update_interval(&self) -> Duration;

	/// Register and spawn metric. Metric is only spawned if it is registered for the first time.
	fn register_and_spawn(self, registry: &Registry) -> Result<(), PrometheusError> {
		match self.register(registry) {
			Ok(()) => {
				self.spawn();
				Ok(())
			},
			Err(PrometheusError::AlreadyReg) => Ok(()),
			Err(e) => Err(e),
		}
	}

	/// Spawn the self update task that will keep update metric value at given intervals.
	fn spawn(self) {
		async_std::task::spawn(async move {
			let update_interval = self.update_interval();
			loop {
				self.update().await;
				async_std::task::sleep(update_interval).await;
			}
		});
	}
}

impl Default for MetricsAddress {
	fn default() -> Self {
		MetricsAddress { host: "127.0.0.1".into(), port: 9616 }
	}
}

impl MetricsParams {
	/// Creates metrics params so that metrics are not exposed.
	pub fn disabled() -> Self {
		MetricsParams { address: None, registry: Registry::new() }
	}

	/// Do not expose metrics.
	pub fn disable(mut self) -> Self {
		self.address = None;
		self
	}
}

impl From<Option<MetricsAddress>> for MetricsParams {
	fn from(address: Option<MetricsAddress>) -> Self {
		MetricsParams { address, registry: Registry::new() }
	}
}

/// Returns metric name optionally prefixed with given prefix.
pub fn metric_name(prefix: Option<&str>, name: &str) -> String {
	if let Some(prefix) = prefix {
		format!("{}_{}", prefix, name)
	} else {
		name.into()
	}
}

/// Set value of gauge metric.
///
/// If value is `Ok(None)` or `Err(_)`, metric would have default value.
pub fn set_gauge_value<T: Default + Debug, V: Atomic<T = T>, E: Debug>(
	gauge: &Gauge<V>,
	value: Result<Option<T>, E>,
) {
	gauge.set(match value {
		Ok(Some(value)) => {
			log::trace!(
				target: "bridge-metrics",
				"Updated value of metric '{:?}': {:?}",
				gauge.desc().first().map(|d| &d.fq_name),
				value,
			);
			value
		},
		Ok(None) => {
			log::warn!(
				target: "bridge-metrics",
				"Failed to update metric '{:?}': value is empty",
				gauge.desc().first().map(|d| &d.fq_name),
			);
			Default::default()
		},
		Err(error) => {
			log::warn!(
				target: "bridge-metrics",
				"Failed to update metric '{:?}': {:?}",
				gauge.desc().first().map(|d| &d.fq_name),
				error,
			);
			Default::default()
		},
	})
}
