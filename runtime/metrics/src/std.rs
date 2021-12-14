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

pub struct Counter;
pub struct CounterVec;

/// Dummy implementation.
impl CounterVec {
	pub fn new(_name: &'static str, _description: &'static str, _labels: &[&'static str]) -> Self {
		CounterVec
	}
	pub fn with_label_values(&mut self, _label_values: &[&'static str]) -> &mut Self {
		self
	}
	pub fn inc_by(&mut self, _: u64) {}
	pub fn inc(&mut self) {}
}

/// Dummy implementation.
impl Counter {
	pub fn new(_name: &'static str, _description: &'static str) -> Self {
		Counter
	}
	pub fn inc_by(&mut self, _: u64) {}
	pub fn inc(&mut self) {}
}
