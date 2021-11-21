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

//! Helpers for tracing memory usage across subsystems.

use metrics::Metrics;
use std::{default::Default, sync::Arc};

use crate::{
	metrics,
	metrics::{
		prometheus,
		prometheus::{CounterVec, Opts, PrometheusError, Registry, U64},
	},
};

const DEFAULT_SUBSYSTEM_NAME: &'static str = "default";
const DEFAULT_SPAN_NAME: &'static str = DEFAULT_SUBSYSTEM_NAME;

/// Metrics wrapper.
#[derive(Default, Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct MemVisorMetrics(pub Option<MemVisorMetricsInner>);

/// Prometheus memory span metrics.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct MemVisorMetricsInner {
	/// Counters for memory cumulative allocation/free.
	/// Labels:
	/// - `span_name`
	/// - `subsystem_name`
	mem_alloc_bytes: CounterVec<U64>,
	mem_free_bytes: CounterVec<U64>,
}

impl Metrics for MemVisorMetrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MemVisorMetricsInner {
			mem_alloc_bytes: prometheus::register(
				CounterVec::new(
					Opts::new("memvisor_alloc_bytes", "Memory footprint increase for a span."),
					&["span_name", "subsystem_name"],
				)?,
				registry,
			)?,
			mem_free_bytes: prometheus::register(
				CounterVec::new(
					Opts::new("memvisor_free_bytes", "Memory footprint decrease for a span."),
					&["span_name", "subsystem_name"],
				)?,
				registry,
			)?,
		};
		Ok(MemVisorMetrics(Some(metrics)))
	}
}

/// Provide APIs to tracking and reporting memory usage.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct MemSpan {
	/// The name of the span.
	name: &'static str,
	/// The name of the subsystem.
	subsystem_name: &'static str,
	/// Current memory usage.
	value: u64,
	/// Ref to `MemVisorMetrics` to enable metric reporting
	/// from inside spans.
	metrics: Arc<MemVisorMetrics>,
}

#[derive(PartialEq)]
#[cfg_attr(test, derive(Debug))]
enum MemSpanEvent {
	TotalAllocated(u64),
	TotalFreed(u64),
}

impl MemSpan {
	/// Creates a child span with a given name.
	pub fn child(&self, name: &'static str) -> MemSpan {
		MemSpan {
			name,
			subsystem_name: self.subsystem_name,
			value: 0,
			metrics: self.metrics.clone(),
		}
	}

	/// Start span and records the initial memory usage for object.
	pub fn start(&mut self, object: &dyn GetMemoryUsage) {
		if self.metrics.0.is_some() {
			self.value = object.memory_usage();
			tracing::debug!(
				target: "memvisor",
				"[START] {}/{} mem usage: {}",
				self.subsystem_name,
				self.name,
				self.value
			);
		}
	}

	/// Ends current span and update metrics.
	pub fn end(&mut self, object: &dyn GetMemoryUsage) {
		if self.metrics.0.is_none() {
			return
		}

		tracing::debug!(
			target: "memvisor",
			"[END] {}/{} mem usage: {}",
			self.subsystem_name,
			self.name,
			object.memory_usage()
		);

		match Self::event(self.value, object.memory_usage()) {
			// `metrics` is guaranteed to be some, unwrap() doesn't panic.
			Some(MemSpanEvent::TotalAllocated(size)) => self
				.metrics
				.0
				.as_ref()
				.unwrap()
				.mem_alloc_bytes
				.with_label_values(&[self.name, self.subsystem_name])
				.inc_by(size),
			Some(MemSpanEvent::TotalFreed(size)) => self
				.metrics
				.0
				.as_ref()
				.unwrap()
				.mem_free_bytes
				.with_label_values(&[self.name, self.subsystem_name])
				.inc_by(size),
			None => {},
		}
	}

	fn event(start: u64, stop: u64) -> Option<MemSpanEvent> {
		if stop > start {
			return Some(MemSpanEvent::TotalAllocated(stop - start))
		} else if stop < start {
			return Some(MemSpanEvent::TotalFreed(start - stop))
		}

		None
	}
}

/// Subsystem name wrapper.
pub enum SubsystemName {
	/// Use default name.
	Default,
	/// Use a specific name.
	Specific(&'static str),
}

impl Into<&'static str> for SubsystemName {
	fn into(self) -> &'static str {
		match self {
			SubsystemName::Specific(subsystem_name) => subsystem_name,
			SubsystemName::Default => DEFAULT_SUBSYSTEM_NAME,
		}
	}
}

/// Abstracts memory usage sampling.
/// For our purpose it doesn't have to be accurate, we are interested in
/// just capturing memory footprint changes.
pub trait GetMemoryUsage {
	/// Returns amount of bytes used.
	fn memory_usage(&self) -> u64;
}

/// Helper that tracks and report memory usage metrics.
pub struct MemVisor {
	/// Metrics for memory usage across all spans.
	metrics: Arc<MemVisorMetrics>,
}

impl MemVisor {
	/// Constructor.
	pub fn new(metrics: MemVisorMetrics) -> Self {
		MemVisor { metrics: Arc::new(metrics) }
	}

	/// Create a new root span.
	pub fn span(&self, subsystem_name: impl Into<&'static str>) -> MemSpan {
		// MemSpan::new(name, self.metrics.clone())
		MemSpan {
			name: DEFAULT_SPAN_NAME,
			subsystem_name: subsystem_name.into(),
			value: 0,
			metrics: self.metrics.clone(),
		}
	}
}

/// Helper macro that wraps code inside a memory span to track a specific object.
#[macro_export]
macro_rules! mem_span {
	(parent($parent: expr) name($name: expr) object($object: expr) $($inner:tt)*) => {
		let mut child_span = $parent.child($name);
		child_span.start($object);
		$($inner)*
		child_span.end($object);
	};
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_mem_span_event() {
		assert_eq!(MemSpan::event(100, 0), Some(MemSpanEvent::TotalFreed(100)));
		assert_eq!(MemSpan::event(0, 100), Some(MemSpanEvent::TotalAllocated(100)));
		assert_eq!(MemSpan::event(1000, 10000), Some(MemSpanEvent::TotalAllocated(9000)));
	}
}
