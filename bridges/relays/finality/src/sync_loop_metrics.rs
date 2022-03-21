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

use relay_utils::metrics::{metric_name, register, IntGauge, Metric, PrometheusError, Registry};

/// Headers sync metrics.
#[derive(Clone)]
pub struct SyncLoopMetrics {
	/// Best syncing header at the source.
	best_source_block_number: IntGauge,
	/// Best syncing header at the target.
	best_target_block_number: IntGauge,
	/// Flag that has `0` value when best source headers at the source node and at-target-chain
	/// are matching and `1` otherwise.
	using_different_forks: IntGauge,
}

impl SyncLoopMetrics {
	/// Create and register headers loop metrics.
	pub fn new(
		prefix: Option<&str>,
		at_source_chain_label: &str,
		at_target_chain_label: &str,
	) -> Result<Self, PrometheusError> {
		Ok(SyncLoopMetrics {
			best_source_block_number: IntGauge::new(
				metric_name(prefix, &format!("best_{}_block_number", at_source_chain_label)),
				format!("Best block number at the {}", at_source_chain_label),
			)?,
			best_target_block_number: IntGauge::new(
				metric_name(prefix, &format!("best_{}_block_number", at_target_chain_label)),
				format!("Best block number at the {}", at_target_chain_label),
			)?,
			using_different_forks: IntGauge::new(
				metric_name(prefix, &format!("is_{}_and_{}_using_different_forks", at_source_chain_label, at_target_chain_label)),
				"Whether the best finalized source block at target node is different (value 1) from the \
				corresponding block at the source node",
			)?,
		})
	}

	/// Returns current value of the using-same-fork flag.
	#[cfg(test)]
	pub(crate) fn is_using_same_fork(&self) -> bool {
		self.using_different_forks.get() == 0
	}

	/// Update best block number at source.
	pub fn update_best_block_at_source<Number: Into<u64>>(&self, source_best_number: Number) {
		self.best_source_block_number.set(source_best_number.into());
	}

	/// Update best block number at target.
	pub fn update_best_block_at_target<Number: Into<u64>>(&self, target_best_number: Number) {
		self.best_target_block_number.set(target_best_number.into());
	}

	/// Update using-same-fork flag.
	pub fn update_using_same_fork(&self, using_same_fork: bool) {
		self.using_different_forks.set(if using_same_fork { 0 } else { 1 })
	}
}

impl Metric for SyncLoopMetrics {
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError> {
		register(self.best_source_block_number.clone(), registry)?;
		register(self.best_target_block_number.clone(), registry)?;
		register(self.using_different_forks.clone(), registry)?;
		Ok(())
	}
}
