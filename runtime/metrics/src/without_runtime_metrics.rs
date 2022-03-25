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

//! Runtime metrics are usable from the wasm runtime only. The purpose of this module is to
//! provide a dummy implementation for the native runtime to avoid cluttering the runtime code
//! with `#[cfg(feature = "runtime-metrics")]`.

use primitives::v2::metric_definitions::{CounterDefinition, CounterVecDefinition};

/// A dummy `Counter`.
pub struct Counter;
/// A dummy `CounterVec`.
pub struct CounterVec;

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
