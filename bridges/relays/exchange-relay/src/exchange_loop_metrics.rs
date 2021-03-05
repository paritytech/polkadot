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

//! Metrics for currency-exchange relay loop.

use crate::exchange::{BlockNumberOf, RelayedBlockTransactions, TransactionProofPipeline};
use relay_utils::metrics::{register, Counter, CounterVec, GaugeVec, Metrics, Opts, Registry, U64};

/// Exchange transactions relay metrics.
#[derive(Clone)]
pub struct ExchangeLoopMetrics {
	/// Best finalized block numbers - "processed" and "known".
	best_block_numbers: GaugeVec<U64>,
	/// Number of processed blocks ("total").
	processed_blocks: Counter<U64>,
	/// Number of processed transactions ("total", "relayed" and "failed").
	processed_transactions: CounterVec<U64>,
}

impl Metrics for ExchangeLoopMetrics {
	fn register(&self, registry: &Registry) -> Result<(), String> {
		register(self.best_block_numbers.clone(), registry).map_err(|e| e.to_string())?;
		register(self.processed_blocks.clone(), registry).map_err(|e| e.to_string())?;
		register(self.processed_transactions.clone(), registry).map_err(|e| e.to_string())?;
		Ok(())
	}
}

impl Default for ExchangeLoopMetrics {
	fn default() -> Self {
		ExchangeLoopMetrics {
			best_block_numbers: GaugeVec::new(
				Opts::new("best_block_numbers", "Best finalized block numbers"),
				&["type"],
			)
			.expect("metric is static and thus valid; qed"),
			processed_blocks: Counter::new("processed_blocks", "Total number of processed blocks")
				.expect("metric is static and thus valid; qed"),
			processed_transactions: CounterVec::new(
				Opts::new("processed_transactions", "Total number of processed transactions"),
				&["type"],
			)
			.expect("metric is static and thus valid; qed"),
		}
	}
}

impl ExchangeLoopMetrics {
	/// Update metrics when single block is relayed.
	pub fn update<P: TransactionProofPipeline>(
		&self,
		best_processed_block_number: BlockNumberOf<P>,
		best_known_block_number: BlockNumberOf<P>,
		relayed_transactions: RelayedBlockTransactions,
	) {
		self.best_block_numbers
			.with_label_values(&["processed"])
			.set(best_processed_block_number.into());
		self.best_block_numbers
			.with_label_values(&["known"])
			.set(best_known_block_number.into());

		self.processed_blocks.inc();

		self.processed_transactions
			.with_label_values(&["total"])
			.inc_by(relayed_transactions.processed as _);
		self.processed_transactions
			.with_label_values(&["relayed"])
			.inc_by(relayed_transactions.relayed as _);
		self.processed_transactions
			.with_label_values(&["failed"])
			.inc_by(relayed_transactions.failed as _);
	}
}
