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

//! Polkadot-to-Kusama headers sync entrypoint.

use async_trait::async_trait;
use relay_kusama_client::Kusama;
use substrate_relay_helper::{finality_pipeline::SubstrateFinalitySyncPipeline, TransactionParams};

/// Maximal saturating difference between `balance(now)` and `balance(now-24h)` to treat
/// relay as gone wild.
///
/// Actual value, returned by `maximal_balance_decrease_per_day_is_sane` test is approximately 0.001
/// KSM, and initial value of this constant was rounded up to 0.1 KSM. But for actual Kusama <>
/// Polkadot deployment we'll be using the same account for delivering finality (free for mandatory
/// headers) and messages. It means that we can't predict maximal loss. But to protect funds against
/// relay/deployment issues, let's limit it so something that is much larger than this estimation -
/// e.g. to 2 KSM.
// TODO: https://github.com/paritytech/parity-bridges-common/issues/1307
pub(crate) const MAXIMAL_BALANCE_DECREASE_PER_DAY: bp_kusama::Balance = 2 * 1_000_000_000_000;

/// Description of Polkadot -> Kusama finalized headers bridge.
#[derive(Clone, Debug)]
pub struct PolkadotFinalityToKusama;
substrate_relay_helper::generate_mocked_submit_finality_proof_call_builder!(
	PolkadotFinalityToKusama,
	PolkadotFinalityToKusamaCallBuilder,
	relay_kusama_client::runtime::Call::BridgePolkadotGrandpa,
	relay_kusama_client::runtime::BridgePolkadotGrandpaCall::submit_finality_proof
);

#[async_trait]
impl SubstrateFinalitySyncPipeline for PolkadotFinalityToKusama {
	type SourceChain = relay_polkadot_client::Polkadot;
	type TargetChain = Kusama;

	type SubmitFinalityProofCallBuilder = PolkadotFinalityToKusamaCallBuilder;
	type TransactionSignScheme = Kusama;

	async fn start_relay_guards(
		target_client: &relay_substrate_client::Client<Kusama>,
		transaction_params: &TransactionParams<sp_core::sr25519::Pair>,
		enable_version_guard: bool,
	) -> relay_substrate_client::Result<()> {
		substrate_relay_helper::finality_guards::start::<Kusama, Kusama>(
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
		// we expect Polkadot -> Kusama relay to be running in mandatory-headers-only mode
		// => we expect single header for every Polkadot session
		let maximal_balance_decrease = compute_maximal_balance_decrease_per_day::<
			bp_kusama::Balance,
			bp_kusama::WeightToFee,
		>(bp_polkadot::DAYS / bp_polkadot::SESSION_LENGTH + 1);
		assert!(
			MAXIMAL_BALANCE_DECREASE_PER_DAY >= maximal_balance_decrease,
			"Maximal expected loss per day {} is larger than hardcoded {}",
			maximal_balance_decrease,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}
}
