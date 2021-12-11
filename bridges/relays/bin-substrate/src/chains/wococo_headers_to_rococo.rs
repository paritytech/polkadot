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

use codec::Encode;
use sp_core::{Bytes, Pair};

use bp_header_chain::justification::GrandpaJustification;
use relay_rococo_client::{Rococo, SigningParams as RococoSigningParams};
use relay_substrate_client::{Client, IndexOf, TransactionSignScheme, UnsignedTransaction};
use relay_utils::metrics::MetricsParams;
use relay_wococo_client::{SyncHeader as WococoSyncHeader, Wococo};
use substrate_relay_helper::finality_pipeline::{
	SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate,
};

/// Maximal saturating difference between `balance(now)` and `balance(now-24h)` to treat
/// relay as gone wild.
///
/// See `maximal_balance_decrease_per_day_is_sane` test for details.
/// Note that this is in plancks, so this corresponds to `1500 UNITS`.
pub(crate) const MAXIMAL_BALANCE_DECREASE_PER_DAY: bp_rococo::Balance = 1_500_000_000_000_000;

/// Wococo-to-Rococo finality sync pipeline.
pub(crate) type FinalityPipelineWococoFinalityToRococo =
	SubstrateFinalityToSubstrate<Wococo, Rococo, RococoSigningParams>;

#[derive(Clone, Debug)]
pub(crate) struct WococoFinalityToRococo {
	finality_pipeline: FinalityPipelineWococoFinalityToRococo,
}

impl WococoFinalityToRococo {
	pub fn new(target_client: Client<Rococo>, target_sign: RococoSigningParams) -> Self {
		Self {
			finality_pipeline: FinalityPipelineWococoFinalityToRococo::new(
				target_client,
				target_sign,
			),
		}
	}
}

impl SubstrateFinalitySyncPipeline for WococoFinalityToRococo {
	type FinalitySyncPipeline = FinalityPipelineWococoFinalityToRococo;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str =
		bp_wococo::BEST_FINALIZED_WOCOCO_HEADER_METHOD;

	type TargetChain = Rococo;

	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		crate::chains::add_polkadot_kusama_price_metrics::<Self::FinalitySyncPipeline>(params)
	}

	fn start_relay_guards(&self) {
		relay_substrate_client::guard::abort_on_spec_version_change(
			self.finality_pipeline.target_client.clone(),
			bp_rococo::VERSION.spec_version,
		);
		relay_substrate_client::guard::abort_when_account_balance_decreased(
			self.finality_pipeline.target_client.clone(),
			self.transactions_author(),
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}

	fn transactions_author(&self) -> bp_rococo::AccountId {
		(*self.finality_pipeline.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		era: bp_runtime::TransactionEraOf<Rococo>,
		transaction_nonce: IndexOf<Rococo>,
		header: WococoSyncHeader,
		proof: GrandpaJustification<bp_wococo::Header>,
	) -> Bytes {
		let call = relay_rococo_client::runtime::Call::BridgeGrandpaWococo(
			relay_rococo_client::runtime::BridgeGrandpaWococoCall::submit_finality_proof(
				Box::new(header.into_inner()),
				proof,
			),
		);
		let genesis_hash = *self.finality_pipeline.target_client.genesis_hash();
		let transaction = Rococo::sign_transaction(
			genesis_hash,
			&self.finality_pipeline.target_sign,
			era,
			UnsignedTransaction::new(call, transaction_nonce),
		);

		Bytes(transaction.encode())
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
