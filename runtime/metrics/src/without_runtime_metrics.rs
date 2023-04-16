// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Runtime metrics are usable from the wasm runtime only. The purpose of this module is to
//! provide a dummy implementation for the native runtime to avoid cluttering the runtime code
//! with `#[cfg(feature = "runtime-metrics")]`.

use primitives::metric_definitions::{
	CounterDefinition, CounterVecDefinition, HistogramDefinition,
};

/// A dummy `Counter`.
pub struct Counter;
/// A dummy `CounterVec`.
pub struct CounterVec;

/// A dummy `Histogram`
pub struct Histogram;

/// Dummy implementation.
impl CounterVec {
	/// Constructor.
	pub const fn new(_definition: CounterVecDefinition) -> Self {
		CounterVec
	}
	/// Sets label values, implementation is a `no op`.
	pub fn with_label_values(&self, _label_values: &[&'static str]) -> &Self {
		self
	}
	/// Increment counter by value, implementation is a `no op`.
	pub fn inc_by(&self, _: u64) {}
	/// Increment counter, implementation is a `no op`.
	pub fn inc(&self) {}
}

/// Dummy implementation.
impl Counter {
	/// Constructor.
	pub const fn new(_definition: CounterDefinition) -> Self {
		Counter
	}
	/// Increment counter by value, implementation is a `no op`.
	pub fn inc_by(&self, _: u64) {}
	/// Increment counter, implementation is a `no op`.
	pub fn inc(&self) {}
}

/// Dummy implementation
impl Histogram {
	/// Create a new histogram as specified by `definition`. This metric needs to be registered
	/// in the client before it can be used.
	pub const fn new(_definition: HistogramDefinition) -> Self {
		Histogram
	}

	// Observe a value in the histogram
	pub fn observe(&self, _value: u128) {}
}

/// Dummy implementation - always 0
pub fn get_current_time() -> u128 {
	0
}
