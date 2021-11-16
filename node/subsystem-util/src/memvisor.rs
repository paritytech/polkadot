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

//! Polkadot Memvisor helper for tracing memory usage across subsystems.

#![forbid(unused_imports)]

use metrics::Metrics;
use std::{sync::Arc};
use std::default::Default;

use crate::{
	metrics,
	metrics::{
		prometheus,
		prometheus::{CounterVec, Opts, PrometheusError, Registry, U64},
	},
};


#[derive(Default, Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct MemVisorMetrics(Option<MemVisorMetricsInner>);

#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct MemVisorMetricsInner {
	/// Counters for memory (de)allocation.
	/// Labels:
	/// - `span_name`
	/// - `object` ... the obejct we are tracking memory for.
	mem_alloc: CounterVec<U64>,
	mem_free: CounterVec<U64>,
}

impl Metrics for MemVisorMetrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MemVisorMetricsInner {
			mem_alloc: prometheus::register(
				CounterVec::new(Opts::new(
						"memvisor_alloc",
						"Memory footprint increase for a span.",
					),
					&["span_name", "object"],
				)?,
				registry,
			)?,
			mem_free: prometheus::register(
				CounterVec::new(
					Opts::new(
						"memvisor_free",
						"Memory footprint decrease for a span.",
					),
					&["span_name", "object"],
				)?,
				registry,
			)?,
		};
		Ok(MemVisorMetrics(Some(metrics)))
	}
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct MemSpan{
	/// The name of the span.
	name: &'static str,
	/// The metric value we are tracking.
	value: u64,
	/// Reference to MemVisorMetrics to enable metric reporting
	/// from inside spans.
	visor_metrics: Arc<MemVisorMetrics>,
}

#[derive(PartialEq)]
#[cfg_attr(test, derive(Debug))]
enum MemSpanEvent {
	None,
	Alloc(u64),
	Free(u64),
}

impl MemSpan {
	pub fn new(
        name: &'static str,
        visor_metrics: Arc<MemVisorMetrics>) -> Self {
		MemSpan {
			name,
			value: 0,
			visor_metrics,
		}
	}

    pub fn child(&self, name: &'static str) -> MemSpan {
        MemSpan {
			name,
			value: 0,
			visor_metrics: self.visor_metrics.clone()
		}
    }

	/// Enable monitoring for a discrete list of suspect data structures.
	/// The total usage should be recorded with this call
	pub fn start(&mut self, start_value: u64) {
		if self.visor_metrics.0.is_some() {
			self.value = start_value;
		}
	}

	/// Ends current span and update metrics.
	pub fn end(&mut self, stop_value: u64) {
		if let Some(metrics) = &self.visor_metrics.0 {
			match Self::event(self.value, stop_value) {
				MemSpanEvent::Alloc(size) => metrics.mem_alloc.with_label_values(&[self.name, "dummy"]).inc_by(size),
				MemSpanEvent::Free(size) => metrics.mem_free.with_label_values(&[self.name, "dummy"]).inc_by(size),
				MemSpanEvent::None => { }
			}
		}
	}

	fn event(start: u64, stop: u64) -> MemSpanEvent {
		if stop > start {
			return MemSpanEvent::Alloc(stop - start)
		} else if stop < start {
			return MemSpanEvent::Free(start - stop)
		}
	
		MemSpanEvent::None
	}
}

pub struct MemVisor {
	/// Root span.
	span: Arc<MemSpan>,
	/// Metrics for memory usage across all spans.
	metrics: Arc<MemVisorMetrics>
}

impl MemVisor {
	// Constructor
	pub fn new(metrics: MemVisorMetrics) -> Self {
		let metrics = Arc::new(metrics);
		MemVisor {
			span: Arc::new(MemSpan::new("root", metrics.clone())),
			metrics
		}
	}

	pub fn span(&self, name: &'static str) -> MemSpan {
		MemSpan::new(name, self.metrics.clone())
	}

	#[cfg(test)]
	pub fn new_dummy() -> MemVisor{
		let metrics = Arc::new(MemVisorMetrics(None));
		MemVisor {
			span: Arc::new(MemSpan::new("root", metrics.clone())),
			metrics,
		}
	}
}



#[cfg(test)]
mod tests {
	use super::*;
	
	#[test]
	fn test_mem_span_event() {
		assert_eq!(MemSpan::event(0, 0), MemSpanEvent::None);
		assert_eq!(MemSpan::event(100, 0), MemSpanEvent::Free(100));
		assert_eq!(MemSpan::event(0, 100), MemSpanEvent::Alloc(100));
		assert_eq!(MemSpan::event(1000, 1000), MemSpanEvent::None);
	}
}