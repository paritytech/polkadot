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

//! Wococo-to-Rococo headers sync entrypoint.

use async_trait::async_trait;
use relay_rococo_client::Rococo;
use substrate_relay_helper::{finality_pipeline::SubstrateFinalitySyncPipeline, TransactionParams};

/// Maximal saturating difference between `balance(now)` and `balance(now-24h)` to treat
/// relay as gone wild.
///
/// See `maximal_balance_decrease_per_day_is_sane` test for details.
/// Note that this is in plancks, so this corresponds to `1500 UNITS`.
pub(crate) const MAXIMAL_BALANCE_DECREASE_PER_DAY: bp_rococo::Balance = 1_500_000_000_000_000;

/// Description of Wococo -> Rococo finalized headers bridge.
#[derive(Clone, Debug)]
pub struct WococoFinalityToRococo;
substrate_relay_helper::generate_mocked_submit_finality_proof_call_builder!(
	WococoFinalityToRococo,
	WococoFinalityToRococoCallBuilder,
	relay_rococo_client::runtime::Call::BridgeGrandpaWococo,
	relay_rococo_client::runtime::BridgeGrandpaWococoCall::submit_finality_proof
);

#[async_trait]
impl SubstrateFinalitySyncPipeline for WococoFinalityToRococo {
	type SourceChain = relay_wococo_client::Wococo;
	type TargetChain = Rococo;

	type SubmitFinalityProofCallBuilder = WococoFinalityToRococoCallBuilder;
	type TransactionSignScheme = Rococo;

	async fn start_relay_guards(
		target_client: &relay_substrate_client::Client<Rococo>,
		transaction_params: &TransactionParams<sp_core::sr25519::Pair>,
		enable_version_guard: bool,
	) -> relay_substrate_client::Result<()> {
		substrate_relay_helper::finality_guards::start::<Rococo, Rococo>(
			target_client,
			transaction_params,
			enable_version_guard,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		)
		.await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::chains::kusama_headers_to_polkadot::tests::compute_maximal_balance_decrease_per_day;

	#[test]
	fn maximal_balance_decrease_per_day_is_sane() {
		// we expect Wococo -> Rococo relay to be running in all-headers mode
		let maximal_balance_decrease = compute_maximal_balance_decrease_per_day::<
			bp_kusama::Balance,
			bp_kusama::WeightToFee,
		>(bp_wococo::DAYS);
		assert!(
			MAXIMAL_BALANCE_DECREASE_PER_DAY >= maximal_balance_decrease,
			"Maximal expected loss per day {} is larger than hardcoded {}",
			maximal_balance_decrease,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}
}
