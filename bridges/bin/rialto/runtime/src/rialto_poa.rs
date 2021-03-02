// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Configuration parameters for the Rialto PoA chain.

use crate::exchange::EthereumTransactionInclusionProof;

use bp_eth_poa::{Address, AuraHeader, RawTransaction, U256};
use bp_header_chain::InclusionProofVerifier;
use frame_support::RuntimeDebug;
use hex_literal::hex;
use pallet_bridge_eth_poa::{
	AuraConfiguration, ChainTime as TChainTime, PruningStrategy as TPruningStrategy, ValidatorsConfiguration,
	ValidatorsSource,
};
use sp_std::prelude::*;

frame_support::parameter_types! {
	pub const FinalityVotesCachingInterval: Option<u64> = Some(8);
	pub BridgeAuraConfiguration: AuraConfiguration =
		aura_configuration();
	pub BridgeValidatorsConfiguration: ValidatorsConfiguration =
		validators_configuration();
}

/// Max number of finalized headers to keep.
const FINALIZED_HEADERS_TO_KEEP: u64 = 5_000;

/// Aura engine configuration for Rialto chain.
pub fn aura_configuration() -> AuraConfiguration {
	AuraConfiguration {
		empty_steps_transition: 0xfffffffff,
		strict_empty_steps_transition: 0,
		validate_step_transition: 0,
		validate_score_transition: 0,
		two_thirds_majority_transition: u64::max_value(),
		min_gas_limit: 0x1388.into(),
		max_gas_limit: U256::max_value(),
		maximum_extra_data_size: 0x20,
	}
}

/// Validators configuration for Rialto PoA chain.
pub fn validators_configuration() -> ValidatorsConfiguration {
	ValidatorsConfiguration::Single(ValidatorsSource::List(genesis_validators()))
}

/// Genesis validators set of Rialto PoA chain.
pub fn genesis_validators() -> Vec<Address> {
	vec![
		hex!("005e714f896a8b7cede9d38688c1a81de72a58e4").into(),
		hex!("007594304039c2937a12220338aab821d819f5a4").into(),
		hex!("004e7a39907f090e19b0b80a277e77b72b22e269").into(),
	]
}

/// Genesis header of the Rialto PoA chain.
///
/// To obtain genesis header from a running node, invoke:
/// ```bash
/// $ http localhost:8545 jsonrpc=2.0 id=1 method=eth_getBlockByNumber params:='["earliest", false]' -v
/// ```
pub fn genesis_header() -> AuraHeader {
	AuraHeader {
		parent_hash: Default::default(),
		timestamp: 0,
		number: 0,
		author: Default::default(),
		transactions_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
		uncles_hash: hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347").into(),
		extra_data: vec![],
		state_root: hex!("a992d04c791620ed7ed96555a80cf0568355bb4bee2656f46899a4372f25f248").into(),
		receipts_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
		log_bloom: Default::default(),
		gas_used: Default::default(),
		gas_limit: 0x222222.into(),
		difficulty: 0x20000.into(),
		seal: vec![vec![0x80], {
			let mut vec = vec![0xb8, 0x41];
			vec.resize(67, 0);
			vec
		}],
	}
}

/// Rialto PoA headers pruning strategy.
///
/// We do not prune unfinalized headers because exchange module only accepts
/// claims from finalized headers. And if we're pruning unfinalized headers, then
/// some claims may never be accepted.
#[derive(Default, RuntimeDebug)]
pub struct PruningStrategy;

impl TPruningStrategy for PruningStrategy {
	fn pruning_upper_bound(&mut self, _best_number: u64, best_finalized_number: u64) -> u64 {
		best_finalized_number.saturating_sub(FINALIZED_HEADERS_TO_KEEP)
	}
}

/// ChainTime provider
#[derive(Default)]
pub struct ChainTime;

impl TChainTime for ChainTime {
	fn is_timestamp_ahead(&self, timestamp: u64) -> bool {
		let now = super::Timestamp::now();
		timestamp > now
	}
}

/// The Rialto PoA Blockchain as seen by the runtime.
pub struct RialtoBlockchain;

impl InclusionProofVerifier for RialtoBlockchain {
	type Transaction = RawTransaction;
	type TransactionInclusionProof = EthereumTransactionInclusionProof;

	fn verify_transaction_inclusion_proof(proof: &Self::TransactionInclusionProof) -> Option<Self::Transaction> {
		let is_transaction_finalized =
			crate::BridgeRialtoPoA::verify_transaction_finalized(proof.block, proof.index, &proof.proof);

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
	fn genesis_hash_matches() {
		assert_eq!(
			genesis_header().compute_hash(),
			hex!("1468e1a0fa20d30025a5a0f87e1cced4fdc393b84b7d2850b11ca5863db482cb").into(),
		);
	}

	#[test]
	fn pruning_strategy_keeps_enough_headers() {
		assert_eq!(
			PruningStrategy::default().pruning_upper_bound(100_000, 1_000),
			0,
			"1_000 <= 5_000 => nothing should be pruned yet",
		);

		assert_eq!(
			PruningStrategy::default().pruning_upper_bound(100_000, 5_000),
			0,
			"5_000 <= 5_000 => nothing should be pruned yet",
		);

		assert_eq!(
			PruningStrategy::default().pruning_upper_bound(100_000, 10_000),
			5_000,
			"5_000 <= 10_000 => we're ready to prune first 5_000 headers",
		);
	}
}
