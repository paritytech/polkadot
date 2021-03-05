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

//! Exchange module complexity is mostly determined by callbacks, defined by runtime.
//! So we are giving runtime opportunity to prepare environment and construct proof
//! before invoking module calls.

use super::{
	Call, Config as CurrencyExchangeConfig, InclusionProofVerifier, Instance, Module as CurrencyExchangeModule,
};
use sp_std::prelude::*;

use frame_benchmarking::{account, benchmarks_instance};
use frame_system::RawOrigin;

const SEED: u32 = 0;
const WORST_TX_SIZE_FACTOR: u32 = 1000;
const WORST_PROOF_SIZE_FACTOR: u32 = 1000;

/// Module we're benchmarking here.
pub struct Module<T: Config<I>, I: Instance>(CurrencyExchangeModule<T, I>);

/// Proof benchmarking parameters.
pub struct ProofParams<Recipient> {
	/// Funds recipient.
	pub recipient: Recipient,
	/// When true, recipient must exists before import.
	pub recipient_exists: bool,
	/// When 0, transaction should have minimal possible size. When this value has non-zero value n,
	/// transaction size should be (if possible) near to MIN_SIZE + n * SIZE_FACTOR.
	pub transaction_size_factor: u32,
	/// When 0, proof should have minimal possible size. When this value has non-zero value n,
	/// proof size should be (if possible) near to MIN_SIZE + n * SIZE_FACTOR.
	pub proof_size_factor: u32,
}

/// Config that must be implemented by runtime.
pub trait Config<I: Instance>: CurrencyExchangeConfig<I> {
	/// Prepare proof for importing exchange transaction.
	fn make_proof(
		proof_params: ProofParams<Self::AccountId>,
	) -> <<Self as CurrencyExchangeConfig<I>>::PeerBlockchain as InclusionProofVerifier>::TransactionInclusionProof;
}

benchmarks_instance! {
	// Benchmark `import_peer_transaction` extrinsic with the best possible conditions:
	// * Proof is the transaction itself.
	// * Transaction has minimal size.
	// * Recipient account exists.
	import_peer_transaction_best_case {
		let i in 1..100;

		let recipient: T::AccountId = account("recipient", i, SEED);
		let proof = T::make_proof(ProofParams {
			recipient: recipient.clone(),
			recipient_exists: true,
			transaction_size_factor: 0,
			proof_size_factor: 0,
		});
	}: import_peer_transaction(RawOrigin::Signed(recipient), proof)

	// Benchmark `import_peer_transaction` extrinsic when recipient account does not exists.
	import_peer_transaction_when_recipient_does_not_exists {
		let i in 1..100;

		let recipient: T::AccountId = account("recipient", i, SEED);
		let proof = T::make_proof(ProofParams {
			recipient: recipient.clone(),
			recipient_exists: false,
			transaction_size_factor: 0,
			proof_size_factor: 0,
		});
	}: import_peer_transaction(RawOrigin::Signed(recipient), proof)

	// Benchmark `import_peer_transaction` when transaction size increases.
	import_peer_transaction_when_transaction_size_increases {
		let i in 1..100;
		let n in 1..WORST_TX_SIZE_FACTOR;

		let recipient: T::AccountId = account("recipient", i, SEED);
		let proof = T::make_proof(ProofParams {
			recipient: recipient.clone(),
			recipient_exists: true,
			transaction_size_factor: n,
			proof_size_factor: 0,
		});
	}: import_peer_transaction(RawOrigin::Signed(recipient), proof)

	// Benchmark `import_peer_transaction` when proof size increases.
	import_peer_transaction_when_proof_size_increases {
		let i in 1..100;
		let n in 1..WORST_PROOF_SIZE_FACTOR;

		let recipient: T::AccountId = account("recipient", i, SEED);
		let proof = T::make_proof(ProofParams {
			recipient: recipient.clone(),
			recipient_exists: true,
			transaction_size_factor: 0,
			proof_size_factor: n,
		});
	}: import_peer_transaction(RawOrigin::Signed(recipient), proof)

	// Benchmark `import_peer_transaction` extrinsic with the worst possible conditions:
	// * Proof is large.
	// * Transaction has large size.
	// * Recipient account does not exists.
	import_peer_transaction_worst_case {
		let i in 1..100;
		let m in WORST_TX_SIZE_FACTOR..WORST_TX_SIZE_FACTOR+1;
		let n in WORST_PROOF_SIZE_FACTOR..WORST_PROOF_SIZE_FACTOR+1;

		let recipient: T::AccountId = account("recipient", i, SEED);
		let proof = T::make_proof(ProofParams {
			recipient: recipient.clone(),
			recipient_exists: false,
			transaction_size_factor: m,
			proof_size_factor: n,
		});
	}: import_peer_transaction(RawOrigin::Signed(recipient), proof)

}
