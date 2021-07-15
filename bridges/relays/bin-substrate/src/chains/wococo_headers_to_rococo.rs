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

use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use relay_rococo_client::{Rococo, SigningParams as RococoSigningParams};
use relay_substrate_client::{Chain, TransactionSignScheme};
use relay_utils::metrics::MetricsParams;
use relay_wococo_client::{SyncHeader as WococoSyncHeader, Wococo};
use sp_core::{Bytes, Pair};

/// Maximal saturating difference between `balance(now)` and `balance(now-24h)` to treat
/// relay as gone wild.
///
/// See `maximal_balance_decrease_per_day_is_sane` test for details.
/// Note that this is in plancks, so this corresponds to `1500 UNITS`.
pub(crate) const MAXIMAL_BALANCE_DECREASE_PER_DAY: bp_rococo::Balance = 1_500_000_000_000_000;

/// Wococo-to-Rococo finality sync pipeline.
pub(crate) type WococoFinalityToRococo = SubstrateFinalityToSubstrate<Wococo, Rococo, RococoSigningParams>;

impl SubstrateFinalitySyncPipeline for WococoFinalityToRococo {
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_wococo::BEST_FINALIZED_WOCOCO_HEADER_METHOD;

	type TargetChain = Rococo;

	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		crate::chains::add_polkadot_kusama_price_metrics::<Self>(params)
	}

	fn start_relay_guards(&self) {
		relay_substrate_client::guard::abort_on_spec_version_change(
			self.target_client.clone(),
			bp_rococo::VERSION.spec_version,
		);
		relay_substrate_client::guard::abort_when_account_balance_decreased(
			self.target_client.clone(),
			self.transactions_author(),
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}

	fn transactions_author(&self) -> bp_rococo::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Rococo as Chain>::Index,
		header: WococoSyncHeader,
		proof: GrandpaJustification<bp_wococo::Header>,
	) -> Bytes {
		let call = relay_rococo_client::runtime::Call::BridgeGrandpaWococo(
			relay_rococo_client::runtime::BridgeGrandpaWococoCall::submit_finality_proof(header.into_inner(), proof),
		);
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Rococo::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);

		Bytes(transaction.encode())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::weights::WeightToFeePolynomial;
	use pallet_bridge_grandpa::weights::WeightInfo;

	#[test]
	fn maximal_balance_decrease_per_day_is_sane() {
		// Rococo/Wococo GRANDPA pallet weights. They're now using Rialto weights => using `RialtoWeight` is justified.
		//
		// Using Rialto runtime this is slightly incorrect, because `DbWeight` of Rococo/Wococo runtime may differ
		// from the `DbWeight` of Rialto runtime. But now (and most probably forever) it is the same.
		type RococoGrandpaPalletWeights = pallet_bridge_grandpa::weights::RialtoWeight<rialto_runtime::Runtime>;

		// The following formula shall not be treated as super-accurate - guard is to protect from mad relays,
		// not to protect from over-average loses.
		//
		// Worst case: we're submitting proof for every source header. Since we submit every header, the number of
		// headers in ancestry proof is near to 0 (let's round up to 2). And the number of authorities is 1024,
		// which is (now) larger than on any existing chain => normally there'll be ~1024*2/3+1 commits.
		const AVG_VOTES_ANCESTRIES_LEN: u32 = 2;
		const AVG_PRECOMMITS_LEN: u32 = 1024 * 2 / 3 + 1;
		let number_of_source_headers_per_day: bp_wococo::Balance = bp_wococo::DAYS as _;
		let single_source_header_submit_call_weight =
			RococoGrandpaPalletWeights::submit_finality_proof(AVG_VOTES_ANCESTRIES_LEN, AVG_PRECOMMITS_LEN);
		// for simplicity - add extra weight for base tx fee + fee that is paid for the tx size + adjusted fee
		let single_source_header_submit_tx_weight = single_source_header_submit_call_weight * 3 / 2;
		let single_source_header_tx_cost = bp_rococo::WeightToFee::calc(&single_source_header_submit_tx_weight);
		let maximal_expected_decrease = single_source_header_tx_cost * number_of_source_headers_per_day;
		assert!(
			MAXIMAL_BALANCE_DECREASE_PER_DAY >= maximal_expected_decrease,
			"Maximal expected loss per day {} is larger than hardcoded {}",
			maximal_expected_decrease,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}
}
