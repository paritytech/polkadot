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

//! A simple implementation of a timer per block hash .
//!
use std::{collections::HashSet, task::Poll, time::Duration};

use futures::{
	future::BoxFuture,
	stream::{FusedStream, FuturesUnordered},
	Stream, StreamExt,
};
use futures_timer::Delay;
use polkadot_primitives::{Hash, ValidatorIndex};
// A list of delayed futures that gets triggered when the waiting time has expired and it is
// time to sign the candidate.
// We have a timer per relay-chain block.
#[derive(Default)]
pub struct SignApprovalsTimers {
	timers: FuturesUnordered<BoxFuture<'static, (Hash, ValidatorIndex)>>,
	blocks: HashSet<Hash>,
}

impl SignApprovalsTimers {
	/// Starts a single timer per block hash
	///
	/// Guarantees that if a timer already exits for the give block hash,
	/// no additional timer is started.
	pub fn maybe_start_timer_for_block(
		&mut self,
		timer_duration_ms: u32,
		block_hash: Hash,
		validator_index: ValidatorIndex,
	) {
		if self.blocks.insert(block_hash) {
			let delay = Delay::new(Duration::from_millis(timer_duration_ms as _));
			self.timers.push(Box::pin(async move {
				delay.await;
				(block_hash, validator_index)
			}));
		}
	}
}

impl Stream for SignApprovalsTimers {
	type Item = (Hash, ValidatorIndex);

	fn poll_next(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Option<Self::Item>> {
		let poll_result = self.timers.poll_next_unpin(cx);
		match poll_result {
			Poll::Ready(Some(result)) => {
				self.blocks.remove(&result.0);
				Poll::Ready(Some(result))
			},
			_ => poll_result,
		}
	}
}

impl FusedStream for SignApprovalsTimers {
	fn is_terminated(&self) -> bool {
		self.timers.is_terminated()
	}
}
