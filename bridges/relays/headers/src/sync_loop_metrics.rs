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

use crate::sync::HeadersSync;
use crate::sync_types::{HeaderStatus, HeadersSyncPipeline};

use num_traits::Zero;
use relay_utils::metrics::{metric_name, register, GaugeVec, Opts, PrometheusError, Registry, U64};

/// Headers sync metrics.
#[derive(Clone)]
pub struct SyncLoopMetrics {
	/// Best syncing headers at "source" and "target" nodes.
	best_block_numbers: GaugeVec<U64>,
	/// Number of headers in given states (see `HeaderStatus`).
	blocks_in_state: GaugeVec<U64>,
}

impl SyncLoopMetrics {
	/// Create and register headers loop metrics.
	pub fn new(registry: &Registry, prefix: Option<&str>) -> Result<Self, PrometheusError> {
		Ok(SyncLoopMetrics {
			best_block_numbers: register(
				GaugeVec::new(
					Opts::new(
						metric_name(prefix, "best_block_numbers"),
						"Best block numbers on source and target nodes",
					),
					&["node"],
				)?,
				registry,
			)?,
			blocks_in_state: register(
				GaugeVec::new(
					Opts::new(
						metric_name(prefix, "blocks_in_state"),
						"Number of blocks in given state",
					),
					&["state"],
				)?,
				registry,
			)?,
		})
	}
}

impl SyncLoopMetrics {
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

	/// Update metrics.
	pub fn update<P: HeadersSyncPipeline>(&self, sync: &HeadersSync<P>) {
		let headers = sync.headers();
		let source_best_number = sync.source_best_number().unwrap_or_else(Zero::zero);
		let target_best_number = sync.target_best_header().map(|id| id.0).unwrap_or_else(Zero::zero);

		self.update_best_block_at_source(source_best_number);
		self.update_best_block_at_target(target_best_number);

		self.blocks_in_state
			.with_label_values(&["maybe_orphan"])
			.set(headers.headers_in_status(HeaderStatus::MaybeOrphan) as _);
		self.blocks_in_state
			.with_label_values(&["orphan"])
			.set(headers.headers_in_status(HeaderStatus::Orphan) as _);
		self.blocks_in_state
			.with_label_values(&["maybe_extra"])
			.set(headers.headers_in_status(HeaderStatus::MaybeExtra) as _);
		self.blocks_in_state
			.with_label_values(&["extra"])
			.set(headers.headers_in_status(HeaderStatus::Extra) as _);
		self.blocks_in_state
			.with_label_values(&["ready"])
			.set(headers.headers_in_status(HeaderStatus::Ready) as _);
		self.blocks_in_state
			.with_label_values(&["incomplete"])
			.set(headers.headers_in_status(HeaderStatus::Incomplete) as _);
		self.blocks_in_state
			.with_label_values(&["submitted"])
			.set(headers.headers_in_status(HeaderStatus::Submitted) as _);
	}
}
