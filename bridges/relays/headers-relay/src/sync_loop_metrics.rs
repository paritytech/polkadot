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

//! Metrics for headers synchronization relay loop.

use crate::sync::HeadersSync;
use crate::sync_types::{HeaderStatus, HeadersSyncPipeline};

use num_traits::Zero;
use relay_utils::metrics::{register, GaugeVec, Metrics, Opts, Registry, U64};

/// Headers sync metrics.
#[derive(Clone)]
pub struct SyncLoopMetrics {
	/// Best syncing headers at "source" and "target" nodes.
	best_block_numbers: GaugeVec<U64>,
	/// Number of headers in given states (see `HeaderStatus`).
	blocks_in_state: GaugeVec<U64>,
}

impl Metrics for SyncLoopMetrics {
	fn register(&self, registry: &Registry) -> Result<(), String> {
		register(self.best_block_numbers.clone(), registry).map_err(|e| e.to_string())?;
		register(self.blocks_in_state.clone(), registry).map_err(|e| e.to_string())?;
		Ok(())
	}
}

impl Default for SyncLoopMetrics {
	fn default() -> Self {
		SyncLoopMetrics {
			best_block_numbers: GaugeVec::new(
				Opts::new("best_block_numbers", "Best block numbers on source and target nodes"),
				&["node"],
			)
			.expect("metric is static and thus valid; qed"),
			blocks_in_state: GaugeVec::new(
				Opts::new("blocks_in_state", "Number of blocks in given state"),
				&["state"],
			)
			.expect("metric is static and thus valid; qed"),
		}
	}
}

impl SyncLoopMetrics {
	/// Update metrics.
	pub fn update<P: HeadersSyncPipeline>(&self, sync: &HeadersSync<P>) {
		let headers = sync.headers();
		let source_best_number = sync.source_best_number().unwrap_or_else(Zero::zero);
		let target_best_number = sync.target_best_header().map(|id| id.0).unwrap_or_else(Zero::zero);

		self.best_block_numbers
			.with_label_values(&["source"])
			.set(source_best_number.into());
		self.best_block_numbers
			.with_label_values(&["target"])
			.set(target_best_number.into());

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
