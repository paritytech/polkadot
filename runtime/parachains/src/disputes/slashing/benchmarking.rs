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

use super::*;

use crate::{disputes::SlashingHandler, initializer, shared};
use frame_benchmarking::{benchmarks, whitelist_account};
use frame_support::traits::{OnFinalize, OnInitialize};
use frame_system::RawOrigin;
use pallet_staking::testing_utils::create_validators;
use primitives::{Hash, PARACHAIN_KEY_TYPE_ID};
use sp_runtime::traits::{One, StaticLookup};
use sp_session::MembershipProof;

// Candidate hash of the disputed candidate.
const CANDIDATE_HASH: CandidateHash = CandidateHash(Hash::zero());

pub trait Config:
	pallet_session::Config
	+ pallet_session::historical::Config
	+ pallet_staking::Config
	+ super::Config
	+ shared::Config
	+ initializer::Config
{
}

fn setup_validator_set<T>(n: u32) -> (SessionIndex, MembershipProof, ValidatorId)
where
	T: Config,
{
	pallet_staking::ValidatorCount::<T>::put(n);

	let balance_factor = 1000;
	// create validators and set random session keys
	for (n, who) in create_validators::<T>(n, balance_factor).unwrap().into_iter().enumerate() {
		use rand::{RngCore, SeedableRng};

		let validator = T::Lookup::lookup(who).unwrap();
		let controller = pallet_staking::Pallet::<T>::bonded(validator).unwrap();

		let keys = {
			const NUM_SESSION_KEYS: usize = 6;
			const SESSION_KEY_LEN: usize = 32;
			let mut keys = [0u8; NUM_SESSION_KEYS * SESSION_KEY_LEN];
			let mut rng = rand_chacha::ChaCha12Rng::seed_from_u64(n as u64);
			rng.fill_bytes(&mut keys);
			keys
		};

		let keys: T::Keys = Decode::decode(&mut &keys[..]).expect("wrong number of session keys?");
		let proof: Vec<u8> = vec![];

		whitelist_account!(controller);
		pallet_session::Pallet::<T>::set_keys(RawOrigin::Signed(controller).into(), keys, proof)
			.expect("session::set_keys should work");
	}

	pallet_session::Pallet::<T>::on_initialize(T::BlockNumber::one());
	initializer::Pallet::<T>::on_initialize(T::BlockNumber::one());
	// skip sessions until the new validator set is enacted
	while pallet_session::Pallet::<T>::validators().len() < n as usize {
		pallet_session::Pallet::<T>::rotate_session();
	}
	initializer::Pallet::<T>::on_finalize(T::BlockNumber::one());

	let session_index = crate::shared::Pallet::<T>::session_index();
	let session_info = crate::session_info::Pallet::<T>::session_info(session_index);
	let session_info = session_info.unwrap();
	let validator_id = session_info.validators.get(ValidatorIndex::from(0)).unwrap().clone();
	let key = (PARACHAIN_KEY_TYPE_ID, validator_id.clone());
	let key_owner_proof = pallet_session::historical::Pallet::<T>::prove(key).unwrap();

	// rotate a session to make sure `key_owner_proof` is historical
	initializer::Pallet::<T>::on_initialize(T::BlockNumber::one());
	pallet_session::Pallet::<T>::rotate_session();
	initializer::Pallet::<T>::on_finalize(T::BlockNumber::one());

	let idx = crate::shared::Pallet::<T>::session_index();
	assert!(
		idx > session_index,
		"session rotation should work for parachain pallets: {} <= {}",
		idx,
		session_index,
	);

	(session_index, key_owner_proof, validator_id)
}

fn setup_dispute<T>(session_index: SessionIndex, validator_id: ValidatorId) -> DisputeProof
where
	T: Config,
{
	let current_session = T::ValidatorSet::session_index();
	assert_ne!(session_index, current_session);

	let validator_index = ValidatorIndex(0);
	let losers = [validator_index].into_iter();
	let backers = losers.clone();

	T::SlashingHandler::punish_for_invalid(session_index, CANDIDATE_HASH, losers, backers);

	let unapplied = <UnappliedSlashes<T>>::get(session_index, CANDIDATE_HASH);
	assert_eq!(unapplied.unwrap().keys.len(), 1);

	dispute_proof(session_index, validator_id, validator_index)
}

fn dispute_proof(
	session_index: SessionIndex,
	validator_id: ValidatorId,
	validator_index: ValidatorIndex,
) -> DisputeProof {
	let kind = SlashingOffenceKind::ForInvalid;
	let time_slot = DisputesTimeSlot::new(session_index, CANDIDATE_HASH);

	DisputeProof { time_slot, kind, validator_index, validator_id }
}

benchmarks! {
	where_clause {
		where T: Config<KeyOwnerProof = MembershipProof>,
	}

	// in this setup we have a single `ForInvalid` dispute
	// submitted for a past session
	report_dispute_lost {
		let n in 4..<<T as super::Config>::BenchmarkingConfig as BenchmarkingConfiguration>::MAX_VALIDATORS;

		let origin = RawOrigin::None.into();
		let (session_index, key_owner_proof, validator_id) = setup_validator_set::<T>(n);
		let dispute_proof = setup_dispute::<T>(session_index, validator_id);
	}: {
		let result = Pallet::<T>::report_dispute_lost_unsigned(
			origin,
			Box::new(dispute_proof),
			key_owner_proof,
		);
		assert!(result.is_ok());
	} verify {
		let unapplied = <UnappliedSlashes<T>>::get(session_index, CANDIDATE_HASH);
		assert!(unapplied.is_none());
	}
}
