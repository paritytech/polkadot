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

use frame_support::storage::{with_transaction_unchecked, TransactionOutcome};
use xcm::latest::prelude::*;
use xcm_executor::traits::ProcessTransaction;

/// Transactional processor implementation using frame transactional layers.
pub struct FrameTransactionalProcessor;
impl ProcessTransaction for FrameTransactionalProcessor {
	const IS_TRANSACTIONAL: bool = true;

	fn process<F>(f: F) -> Result<(), XcmError>
	where
		F: FnOnce() -> Result<(), XcmError>,
	{
		// Is safe to use because of the `MAX_XCM_DECODE_DEPTH`.
		with_transaction_unchecked(|| -> TransactionOutcome<Result<(), XcmError>> {
			let outcome = f();
			match &outcome {
				Ok(_) => TransactionOutcome::Commit(Ok(())),
				Err(err) => TransactionOutcome::Rollback(Err(*err)),
			}
		})
	}
}
