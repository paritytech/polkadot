// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Prometheus metrics related to undying collator.

pub use polkadot_node_metrics::metrics::{self, prometheus, Metrics as MetricsTrait};
use std::fmt;

/// Collator Prometheus metrics.
#[derive(Clone)]
struct MetricsInner {
	hrmp_messages_sent: prometheus::CounterVec<prometheus::U64>,
	hrmp_messages_received: prometheus::CounterVec<prometheus::U64>,
	hrmp_bytes_sent: prometheus::CounterVec<prometheus::U64>,
	hrmp_bytes_received: prometheus::CounterVec<prometheus::U64>,
}

/// A shareable metrics type
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub(crate) fn on_hrmp_inbound(&self, destination: u32, size: usize) {
		if let Some(metrics) = &self.0 {
			let dest_para_string = destination.to_string();
			metrics
				.hrmp_messages_received
				.with_label_values(&[dest_para_string.as_str()])
				.inc();
			metrics
				.hrmp_bytes_received
				.with_label_values(&[dest_para_string.as_str()])
				.inc_by(size as u64);
		}
	}

	pub(crate) fn on_hrmp_outbound(&self, source: u32, size: usize) {
		if let Some(metrics) = &self.0 {
			let source_para_string = source.to_string();
			metrics
				.hrmp_messages_sent
				.with_label_values(&[source_para_string.as_str()])
				.inc();
			metrics
				.hrmp_bytes_sent
				.with_label_values(&[source_para_string.as_str()])
				.inc_by(size as u64);
		}
	}
}

impl MetricsTrait for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			hrmp_messages_received: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_undying_hrmp_messages_received",
						"Number of HRMP messages received",
					),
					&["peer"],
				)?,
				registry,
			)?,
			hrmp_bytes_received: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_undying_hrmp_bytes_sent",
						"Total size of HRMP messages sent",
					),
					&["peer"],
				)?,
				registry,
			)?,
			hrmp_messages_sent: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_undying_hrmp_messages_sent",
						"Number of HRMP messages sent",
					),
					&["peer"],
				)?,
				registry,
			)?,
			hrmp_bytes_sent: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_undying_hrmp_bytes_received",
						"Total size of HRMP messages received",
					),
					&["peer"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

impl fmt::Debug for Metrics {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("Metrics {{...}}")
	}
}
