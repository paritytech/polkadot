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

use crate::exchange::EthereumTransactionInclusionProof;

use bp_eth_poa::{Address, AuraHeader, RawTransaction, U256};
use bp_header_chain::InclusionProofVerifier;
use frame_support::RuntimeDebug;
use hex_literal::hex;
use pallet_bridge_eth_poa::{
	AuraConfiguration, ChainTime as TChainTime, PruningStrategy as BridgePruningStrategy, ValidatorsConfiguration,
	ValidatorsSource,
};
use sp_std::prelude::*;

frame_support::parameter_types! {
	pub const FinalityVotesCachingInterval: Option<u64> = Some(16);
	pub BridgeAuraConfiguration: AuraConfiguration =
		kovan_aura_configuration();
	pub BridgeValidatorsConfiguration: ValidatorsConfiguration =
		kovan_validators_configuration();
}

/// Max number of finalized headers to keep. It is equivalent of ~24 hours of
/// finalized blocks on current Kovan chain.
const FINALIZED_HEADERS_TO_KEEP: u64 = 20_000;

/// Aura engine configuration for Kovan chain.
pub fn kovan_aura_configuration() -> AuraConfiguration {
	AuraConfiguration {
		empty_steps_transition: u64::max_value(),
		strict_empty_steps_transition: 0,
		validate_step_transition: 0x16e360,
		validate_score_transition: 0x41a3c4,
		two_thirds_majority_transition: u64::max_value(),
		min_gas_limit: 0x1388.into(),
		max_gas_limit: U256::max_value(),
		maximum_extra_data_size: 0x20,
	}
}

/// Validators configuration for Kovan chain.
pub fn kovan_validators_configuration() -> ValidatorsConfiguration {
	ValidatorsConfiguration::Multi(vec![
		(0, ValidatorsSource::List(genesis_validators())),
		(
			10960440,
			ValidatorsSource::List(vec![
				hex!("00D6Cc1BA9cf89BD2e58009741f4F7325BAdc0ED").into(),
				hex!("0010f94b296a852aaac52ea6c5ac72e03afd032d").into(),
				hex!("00a0a24b9f0e5ec7aa4c7389b8302fd0123194de").into(),
			]),
		),
		(
			10960500,
			ValidatorsSource::Contract(
				hex!("aE71807C1B0a093cB1547b682DC78316D945c9B8").into(),
				vec![
					hex!("d05f7478c6aa10781258c5cc8b4f385fc8fa989c").into(),
					hex!("03801efb0efe2a25ede5dd3a003ae880c0292e4d").into(),
					hex!("a4df255ecf08bbf2c28055c65225c9a9847abd94").into(),
					hex!("596e8221a30bfe6e7eff67fee664a01c73ba3c56").into(),
					hex!("faadface3fbd81ce37b0e19c0b65ff4234148132").into(),
				],
			),
		),
	])
}

/// Genesis validators set of Kovan chain.
pub fn genesis_validators() -> Vec<Address> {
	vec![
		hex!("00D6Cc1BA9cf89BD2e58009741f4F7325BAdc0ED").into(),
		hex!("00427feae2419c15b89d1c21af10d1b6650a4d3d").into(),
		hex!("4Ed9B08e6354C70fE6F8CB0411b0d3246b424d6c").into(),
		hex!("0020ee4Be0e2027d76603cB751eE069519bA81A1").into(),
		hex!("0010f94b296a852aaac52ea6c5ac72e03afd032d").into(),
		hex!("007733a1FE69CF3f2CF989F81C7b4cAc1693387A").into(),
		hex!("00E6d2b931F55a3f1701c7389d592a7778897879").into(),
		hex!("00e4a10650e5a6D6001C38ff8E64F97016a1645c").into(),
		hex!("00a0a24b9f0e5ec7aa4c7389b8302fd0123194de").into(),
	]
}

/// Genesis header of the Kovan chain.
pub fn genesis_header() -> AuraHeader {
	AuraHeader {
		parent_hash: Default::default(),
		timestamp: 0,
		number: 0,
		author: Default::default(),
		transactions_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
		uncles_hash: hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347").into(),
		extra_data: vec![],
		state_root: hex!("2480155b48a1cea17d67dbfdfaafe821c1d19cdd478c5358e8ec56dec24502b2").into(),
		receipts_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
		log_bloom: Default::default(),
		gas_used: Default::default(),
		gas_limit: 6000000.into(),
		difficulty: 131072.into(),
		seal: vec![
			vec![128],
			vec![
				184, 65, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
				0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
			],
		],
	}
}

/// Kovan headers pruning strategy.
///
/// We do not prune unfinalized headers because exchange module only accepts
/// claims from finalized headers. And if we're pruning unfinalized headers, then
/// some claims may never be accepted.
#[derive(Default, RuntimeDebug)]
pub struct PruningStrategy;

impl BridgePruningStrategy for PruningStrategy {
	fn pruning_upper_bound(&mut self, _best_number: u64, best_finalized_number: u64) -> u64 {
		best_finalized_number.saturating_sub(FINALIZED_HEADERS_TO_KEEP)
	}
}

/// PoA Header timestamp verification against `Timestamp` pallet.
#[derive(Default, RuntimeDebug)]
pub struct ChainTime;

impl TChainTime for ChainTime {
	fn is_timestamp_ahead(&self, timestamp: u64) -> bool {
		let now = super::Timestamp::now();
		timestamp > now
	}
}

/// The Kovan Blockchain as seen by the runtime.
pub struct KovanBlockchain;

impl InclusionProofVerifier for KovanBlockchain {
	type Transaction = RawTransaction;
	type TransactionInclusionProof = EthereumTransactionInclusionProof;

	fn verify_transaction_inclusion_proof(proof: &Self::TransactionInclusionProof) -> Option<Self::Transaction> {
		let is_transaction_finalized =
			crate::BridgeKovan::verify_transaction_finalized(proof.block, proof.index, &proof.proof);

		if !is_transaction_finalized {
			return None;
		}

		proof.proof.get(proof.index as usize).map(|(tx, _)| tx.clone())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pruning_strategy_keeps_enough_headers() {
		assert_eq!(
			PruningStrategy::default().pruning_upper_bound(100_000, 10_000),
			0,
			"10_000 <= 20_000 => nothing should be pruned yet",
		);

		assert_eq!(
			PruningStrategy::default().pruning_upper_bound(100_000, 20_000),
			0,
			"20_000 <= 20_000 => nothing should be pruned yet",
		);

		assert_eq!(
			PruningStrategy::default().pruning_upper_bound(100_000, 30_000),
			10_000,
			"20_000 <= 30_000 => we're ready to prune first 10_000 headers",
		);
	}
}
