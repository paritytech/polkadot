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

//! Metrics for headers synchronization relay loop.

use relay_utils::metrics::{
	metric_name, register, GaugeVec, Metric, Opts, PrometheusError, Registry, U64,
};

/// Headers sync metrics.
#[derive(Clone)]
pub struct SyncLoopMetrics {
	/// Best syncing headers at "source" and "target" nodes.
	best_block_numbers: GaugeVec<U64>,
}

impl SyncLoopMetrics {
	/// Create and register headers loop metrics.
	pub fn new(prefix: Option<&str>) -> Result<Self, PrometheusError> {
		Ok(SyncLoopMetrics {
			best_block_numbers: GaugeVec::new(
				Opts::new(
					metric_name(prefix, "best_block_numbers"),
					"Best block numbers on source and target nodes",
				),
				&["node"],
			)?,
		})
	}

	/// Update best block number at source.
	pub fn update_best_block_at_source<Number: Into<u64>>(&self, source_best_number: Number) {
		self.best_block_numbers
			.with_label_values(&["source"])
			.set(source_best_number.into());
	}

	/// Update best block number at target.
	pub fn update_best_block_at_target<Number: Into<u64>>(&self, target_best_number: Number) {
		self.best_block_numbers
			.with_label_values(&["target"])
			.set(target_best_number.into());
	}
}

impl Metric for SyncLoopMetrics {
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError> {
		register(self.best_block_numbers.clone(), registry)?;
		Ok(())
	}
}
