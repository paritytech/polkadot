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

//! Kusama-to-Polkadot headers sync entrypoint.

use async_trait::async_trait;
use relay_polkadot_client::Polkadot;
use substrate_relay_helper::{finality_pipeline::SubstrateFinalitySyncPipeline, TransactionParams};

/// Maximal saturating difference between `balance(now)` and `balance(now-24h)` to treat
/// relay as gone wild.
///
/// Actual value, returned by `maximal_balance_decrease_per_day_is_sane` test is approximately 21
/// DOT, and initial value of this constant was rounded up to 30 DOT. But for actual Kusama <>
/// Polkadot deployment we'll be using the same account for delivering finality (free for mandatory
/// headers) and messages. It means that we can't predict maximal loss. But to protect funds against
/// relay/deployment issues, let's limit it so something that is much larger than this estimation -
/// e.g. to 100 DOT.
// TODO: https://github.com/paritytech/parity-bridges-common/issues/1307
pub(crate) const MAXIMAL_BALANCE_DECREASE_PER_DAY: bp_polkadot::Balance = 100 * 10_000_000_000;

/// Description of Kusama -> Polkadot finalized headers bridge.
#[derive(Clone, Debug)]
pub struct KusamaFinalityToPolkadot;
substrate_relay_helper::generate_mocked_submit_finality_proof_call_builder!(
	KusamaFinalityToPolkadot,
	KusamaFinalityToPolkadotCallBuilder,
	relay_polkadot_client::runtime::Call::BridgeKusamaGrandpa,
	relay_polkadot_client::runtime::BridgeKusamaGrandpaCall::submit_finality_proof
);

#[async_trait]
impl SubstrateFinalitySyncPipeline for KusamaFinalityToPolkadot {
	type SourceChain = relay_kusama_client::Kusama;
	type TargetChain = Polkadot;

	type SubmitFinalityProofCallBuilder = KusamaFinalityToPolkadotCallBuilder;
	type TransactionSignScheme = Polkadot;

	async fn start_relay_guards(
		target_client: &relay_substrate_client::Client<Polkadot>,
		transaction_params: &TransactionParams<sp_core::sr25519::Pair>,
		enable_version_guard: bool,
	) -> relay_substrate_client::Result<()> {
		substrate_relay_helper::finality_guards::start::<Polkadot, Polkadot>(
			target_client,
			transaction_params,
			enable_version_guard,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		)
		.await
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use frame_support::weights::WeightToFeePolynomial;
	use pallet_bridge_grandpa::weights::WeightInfo;

	pub fn compute_maximal_balance_decrease_per_day<B, W>(expected_source_headers_per_day: u32) -> B
	where
		B: From<u32> + std::ops::Mul<Output = B>,
		W: WeightToFeePolynomial<Balance = B>,
	{
		// we assume that the GRANDPA is not lagging here => ancestry length will be near to 0
		// (let's round up to 2)
		const AVG_VOTES_ANCESTRIES_LEN: u32 = 2;
		// let's assume number of validators is 1024 (more than on any existing well-known chain
		// atm) => number of precommits is *2/3 + 1
		const AVG_PRECOMMITS_LEN: u32 = 1024 * 2 / 3 + 1;

		// GRANDPA pallet weights. We're now using Rialto weights everywhere.
		//
		// Using Rialto runtime is slightly incorrect, because `DbWeight` of other runtimes may
		// differ from the `DbWeight` of Rialto runtime. But now (and most probably forever) it is
		// the same.
		type GrandpaPalletWeights =
			pallet_bridge_grandpa::weights::MillauWeight<rialto_runtime::Runtime>;

		// The following formula shall not be treated as super-accurate - guard is to protect from
		// mad relays, not to protect from over-average loses.

		// increase number of headers a bit
		let expected_source_headers_per_day = expected_source_headers_per_day * 110 / 100;
		let single_source_header_submit_call_weight = GrandpaPalletWeights::submit_finality_proof(
			AVG_VOTES_ANCESTRIES_LEN,
			AVG_PRECOMMITS_LEN,
		);
		// for simplicity - add extra weight for base tx fee + fee that is paid for the tx size +
		// adjusted fee
		let single_source_header_submit_tx_weight = single_source_header_submit_call_weight * 3 / 2;
		let single_source_header_tx_cost = W::weight_to_fee(&single_source_header_submit_tx_weight);
		single_source_header_tx_cost * B::from(expected_source_headers_per_day)
	}

	#[test]
	fn maximal_balance_decrease_per_day_is_sane() {
		// we expect Kusama -> Polkadot relay to be running in mandatory-headers-only mode
		// => we expect single header for every Kusama session
		let maximal_balance_decrease = compute_maximal_balance_decrease_per_day::<
			bp_polkadot::Balance,
			bp_polkadot::WeightToFee,
		>(bp_kusama::DAYS / bp_kusama::SESSION_LENGTH + 1);
		assert!(
			MAXIMAL_BALANCE_DECREASE_PER_DAY >= maximal_balance_decrease,
			"Maximal expected loss per day {} is larger than hardcoded {}",
			maximal_balance_decrease,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}
}
