// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

pub use substrate_prometheus_endpoint::{register, Counter, CounterVec, Gauge, GaugeVec, Opts, Registry, F64, U64};

use async_std::sync::{Arc, Mutex};
use std::net::SocketAddr;
use substrate_prometheus_endpoint::init_prometheus;
use sysinfo::{ProcessExt, RefreshKind, System, SystemExt};

/// Prometheus endpoint MetricsParams.
#[derive(Debug, Clone)]
pub struct MetricsParams {
	/// Serve HTTP requests at given host.
	pub host: String,
	/// Serve HTTP requests at given port.
	pub port: u16,
}

/// Metrics API.
pub trait Metrics {
	/// Register metrics in the registry.
	fn register(&self, registry: &Registry) -> Result<(), String>;
}

/// Global Prometheus metrics.
#[derive(Debug, Clone)]
pub struct GlobalMetrics {
	system: Arc<Mutex<System>>,
	system_average_load: GaugeVec<F64>,
	process_cpu_usage_percentage: Gauge<F64>,
	process_memory_usage_bytes: Gauge<U64>,
}

/// Start Prometheus endpoint with given metrics registry.
pub fn start(
	prefix: String,
	params: Option<MetricsParams>,
	global_metrics: &GlobalMetrics,
	extra_metrics: &impl Metrics,
) {
	let params = match params {
		Some(params) => params,
		None => return,
	};

	assert!(!prefix.is_empty(), "Metrics prefix can not be empty");

	let do_start = move || {
		let prometheus_socket_addr = SocketAddr::new(
			params
				.host
				.parse()
				.map_err(|err| format!("Invalid Prometheus host {}: {}", params.host, err))?,
			params.port,
		);
		let metrics_registry =
			Registry::new_custom(Some(prefix), None).expect("only fails if prefix is empty; prefix is not empty; qed");
		global_metrics.register(&metrics_registry)?;
		extra_metrics.register(&metrics_registry)?;
		async_std::task::spawn(async move {
			init_prometheus(prometheus_socket_addr, metrics_registry)
				.await
				.map_err(|err| format!("Error starting Prometheus endpoint: {}", err))
		});

		Ok(())
	};

	let result: Result<(), String> = do_start();
	if let Err(err) = result {
		log::warn!(
			target: "bridge",
			"Failed to expose metrics: {}",
			err,
		);
	}
}

impl Default for MetricsParams {
	fn default() -> Self {
		MetricsParams {
			host: "127.0.0.1".into(),
			port: 9616,
		}
	}
}

impl Metrics for GlobalMetrics {
	fn register(&self, registry: &Registry) -> Result<(), String> {
		register(self.system_average_load.clone(), registry).map_err(|e| e.to_string())?;
		register(self.process_cpu_usage_percentage.clone(), registry).map_err(|e| e.to_string())?;
		register(self.process_memory_usage_bytes.clone(), registry).map_err(|e| e.to_string())?;
		Ok(())
	}
}

impl Default for GlobalMetrics {
	fn default() -> Self {
		GlobalMetrics {
			system: Arc::new(Mutex::new(System::new_with_specifics(RefreshKind::everything()))),
			system_average_load: GaugeVec::new(Opts::new("system_average_load", "System load average"), &["over"])
				.expect("metric is static and thus valid; qed"),
			process_cpu_usage_percentage: Gauge::new("process_cpu_usage_percentage", "Process CPU usage")
				.expect("metric is static and thus valid; qed"),
			process_memory_usage_bytes: Gauge::new(
				"process_memory_usage_bytes",
				"Process memory (resident set size) usage",
			)
			.expect("metric is static and thus valid; qed"),
		}
	}
}

impl GlobalMetrics {
	/// Update metrics.
	pub async fn update(&self) {
		// update system-wide metrics
		let mut system = self.system.lock().await;
		let load = system.get_load_average();
		self.system_average_load.with_label_values(&["1min"]).set(load.one);
		self.system_average_load.with_label_values(&["5min"]).set(load.five);
		self.system_average_load.with_label_values(&["15min"]).set(load.fifteen);

		// update process-related metrics
		let pid = sysinfo::get_current_pid().expect(
			"only fails where pid is unavailable (os=unknown || arch=wasm32);\
				relay is not supposed to run in such MetricsParamss;\
				qed",
		);
		let is_process_refreshed = system.refresh_process(pid);
		match (is_process_refreshed, system.get_process(pid)) {
			(true, Some(process_info)) => {
				let cpu_usage = process_info.cpu_usage() as f64;
				let memory_usage = process_info.memory() * 1024;
				log::trace!(
					target: "bridge-metrics",
					"Refreshed process metrics: CPU={}, memory={}",
					cpu_usage,
					memory_usage,
				);

				self.process_cpu_usage_percentage
					.set(if cpu_usage.is_finite() { cpu_usage } else { 0f64 });
				self.process_memory_usage_bytes.set(memory_usage);
			}
			_ => {
				log::warn!(
					target: "bridge",
					"Failed to refresh process information. Metrics may show obsolete values",
				);
			}
		}
	}
}
