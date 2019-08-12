// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! A module for tracking all attestations that fell on a given candidate receipt.
//!
//! In the future, it is planned that this module will handle dispute resolution
//! as well.

use rstd::prelude::*;
use codec::{Encode, Decode};
use srml_support::{decl_storage, decl_module, ensure};

use primitives::{Hash, parachain::{AttestedCandidate, CandidateReceipt, Id as ParaId}};
use {system, session::{self, SessionIndex}};
use srml_support::{
	StorageValue, StorageMap, StorageDoubleMap, dispatch::Result, traits::Get,
};

use inherents::{ProvideInherent, InherentData, RuntimeString, MakeFatalError, InherentIdentifier};
use system::ensure_none;

/// Parachain blocks included in a recent relay-chain block.
#[derive(Encode, Decode)]
pub struct IncludedBlocks<T: Trait> {
	/// The actual relay chain block number where blocks were included.
	pub actual_number: T::BlockNumber,
	/// The session index at this block.
	pub session: SessionIndex,
	/// The randomness seed at this block.
	pub random_seed: [u8; 32],
	/// All parachain IDs active at this block.
	pub active_parachains: Vec<ParaId>,
	/// Hashes of the parachain candidates included at this block.
	pub para_blocks: Vec<Hash>,
}

/// Attestations kept over time on a parachain block.
#[derive(Encode, Decode)]
pub struct BlockAttestations<T: Trait> {
	receipt: CandidateReceipt,
	valid: Vec<T::AccountId>, // stash account ID of voter.
	invalid: Vec<T::AccountId>, // stash account ID of voter.
}

/// Additional attestations on a parachain block, after it was included.
#[derive(Encode, Decode, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct MoreAttestations;

pub trait Trait: session::Trait {
	/// How many blocks ago we're willing to accept attestations for.
	type AttestationPeriod: Get<Self::BlockNumber>;

	/// Get a list of the validators' underlying identities.
	type ValidatorIdentities: Get<Vec<Self::AccountId>>;
}

decl_storage! {
	trait Store for Module<T: Trait> as Attestations {
		/// A mapping from modular block number (n % AttestationPeriod)
		/// to session index and the list of candidate hashes.
		pub RecentParaBlocks: map T::BlockNumber => Option<IncludedBlocks<T>>;

		/// Attestations on a recent parachain block.
		pub ParaBlockAttestations: double_map T::BlockNumber, blake2_128(Hash) => Option<BlockAttestations<T>>;

		// Did we already have more attestations included in this block?
		DidUpdate: bool;
	}
}

decl_module! {
	/// Parachain-attestations module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		/// Provide candidate receipts for parachains, in ascending order by id.
		fn more_attestations(origin, _more: MoreAttestations) -> Result {
			ensure_none(origin)?;
			ensure!(!<DidUpdate>::exists(), "More attestations can be added only once in a block.");
			<DidUpdate>::put(true);

			Ok(())
		}

		fn on_finalize(_n: T::BlockNumber) {
			<DidUpdate>::kill();
		}
	}
}

impl<T: Trait> Module<T> {
	/// Update recent candidates to contain the already-checked parachain candidates.
	pub(crate) fn note_included(heads: &[AttestedCandidate], para_blocks: IncludedBlocks<T>) {
		let attestation_period = T::AttestationPeriod::get();
		let mod_num = para_blocks.actual_number % attestation_period;

		// clear old entry that was in this place.
		if let Some(old_entry) = <RecentParaBlocks<T>>::take(&mod_num) {
			<ParaBlockAttestations<T>>::remove_prefix(&old_entry.actual_number);
		}

		let validators = T::ValidatorIdentities::get();

		// make new entry.
		for (head, hash) in heads.iter().zip(&para_blocks.para_blocks) {
			let mut valid = Vec::new();
			let invalid = Vec::new();

			for (auth_index, _) in head.validator_indices
				.iter()
				.enumerate()
				.filter(|(_, bit)| *bit)
			{
				let stash_id = validators.get(auth_index)
					.expect("auth_index checked to be within bounds in `check_candidates`; qed")
					.clone();

				valid.push(stash_id);
			}

			let summary = BlockAttestations {
				receipt: head.candidate().clone(),
				valid,
				invalid,
			};

			<ParaBlockAttestations<T>>::insert(&para_blocks.actual_number, hash, &summary);
		}

		<RecentParaBlocks<T>>::insert(&mod_num, &para_blocks);
	}
}

/// An identifier for inherent data that provides after-the-fact attestations
/// on already included parachain blocks.
pub const MORE_ATTESTATIONS_IDENTIFIER: InherentIdentifier = *b"par-atts";

pub type InherentType = MoreAttestations;

impl<T: Trait> ProvideInherent for Module<T> {
	type Call = Call<T>;
	type Error = MakeFatalError<RuntimeString>;
	const INHERENT_IDENTIFIER: InherentIdentifier = MORE_ATTESTATIONS_IDENTIFIER;

	fn create_inherent(data: &InherentData) -> Option<Self::Call> {
		data.get_data::<InherentType>(&MORE_ATTESTATIONS_IDENTIFIER)
			.ok()
			.and_then(|x| x.map(Call::more_attestations))
	}
}
