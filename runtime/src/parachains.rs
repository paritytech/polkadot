// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! Main parachains logic. For now this is just the determination of which validators do what.

use rstd::prelude::*;
use codec::Decode;

use sr_primitives::{RuntimeString, traits::{Extrinsic, Block as BlockT,
	Hash, BlakeTwo256, ProvideInherent}};
use primitives::parachain::{Id, Chain, DutyRoster, CandidateReceipt};
use {system, session};

use srml_support::{StorageValue, StorageMap};
use srml_support::dispatch::Result;

#[cfg(any(feature = "std", test))]
use sr_primitives::{self, ChildrenStorageMap};

use system::ensure_inherent;

pub trait Trait: session::Trait {
	/// The position of the set_heads call in the block.
	const SET_POSITION: u32;
}

decl_storage! {
	trait Store for Module<T: Trait> as Parachains {
		// Vector of all parachain IDs.
		pub Parachains get(active_parachains): Vec<Id>;
		// The parachains registered at present.
		pub Code get(parachain_code): map Id => Option<Vec<u8>>;
		// The heads of the parachains registered at present. these are kept sorted.
		pub Heads get(parachain_head): map Id => Option<Vec<u8>>;

		// Did the parachain heads get updated in this block?
		DidUpdate: bool;
	}
	add_extra_genesis {
		config(parachains): Vec<(Id, Vec<u8>, Vec<u8>)>;
		build(|storage: &mut sr_primitives::StorageMap, _: &mut ChildrenStorageMap, config: &GenesisConfig<T>| {
			use codec::Encode;

			let mut p = config.parachains.clone();
			p.sort_unstable_by_key(|&(ref id, _, _)| id.clone());
			p.dedup_by_key(|&mut (ref id, _, _)| id.clone());

			let only_ids: Vec<_> = p.iter().map(|&(ref id, _, _)| id).cloned().collect();

			storage.insert(GenesisConfig::<T>::hash(<Parachains<T>>::key()).to_vec(), only_ids.encode());

			for (id, code, genesis) in p {
				let code_key = GenesisConfig::<T>::hash(&<Code<T>>::key_for(&id)).to_vec();
				let head_key = GenesisConfig::<T>::hash(&<Heads<T>>::key_for(&id)).to_vec();

				storage.insert(code_key, code.encode());
				storage.insert(head_key, genesis.encode());
			}
		});
	}
}

decl_module! {
	/// Parachains module.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		/// Provide candidate receipts for parachains, in ascending order by id.
		fn set_heads(origin, heads: Vec<CandidateReceipt>) -> Result {
			ensure_inherent(origin)?;
			ensure!(!<DidUpdate<T>>::exists(), "Parachain heads must be updated only once in the block");
			ensure!(
				<system::Module<T>>::extrinsic_index() == Some(T::SET_POSITION),
				"Parachain heads update extrinsic must be at position {} in the block"
	//			, T::SET_POSITION
			);

			let active_parachains = Self::active_parachains();

			// perform integrity checks before writing to storage.
			{
				let n_parachains = active_parachains.len();
				ensure!(heads.len() <= n_parachains, "Too many parachain candidates");

				let mut last_id = None;
				let mut iter = active_parachains.iter();
				for head in &heads {
					// proposed heads must be ascending order by parachain ID without duplicate.
					ensure!(
						last_id.as_ref().map_or(true, |x| x < &head.parachain_index),
						"Parachain candidates out of order by ID"
					);

					// must be unknown since active parachains are always sorted.
					ensure!(
						iter.find(|x| x == &&head.parachain_index).is_some(),
						"Submitted candidate for unregistered or out-of-order parachain {}"
					);

					last_id = Some(head.parachain_index);
				}
			}

			for head in heads {
				let id = head.parachain_index.clone();
				<Heads<T>>::insert(id, head.head_data.0);
			}

			<DidUpdate<T>>::put(true);

			Ok(())
		}

		/// Register a parachain with given code.
		/// Fails if given ID is already used.
		pub fn register_parachain(id: Id, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result {
			let mut parachains = Self::active_parachains();
			match parachains.binary_search(&id) {
				Ok(_) => fail!("Parachain already exists"),
				Err(idx) => parachains.insert(idx, id),
			}

			<Code<T>>::insert(id, code);
			<Parachains<T>>::put(parachains);
			<Heads<T>>::insert(id, initial_head_data);

			Ok(())
		}

		/// Deregister a parachain with given id
		pub fn deregister_parachain(id: Id) -> Result {
			let mut parachains = Self::active_parachains();
			match parachains.binary_search(&id) {
				Ok(idx) => { parachains.remove(idx); }
				Err(_) => {}
			}

			<Code<T>>::remove(id);
			<Heads<T>>::remove(id);
			<Parachains<T>>::put(parachains);
			Ok(())
		}

		fn on_finalise(_n: T::BlockNumber) {
			assert!(<Self as Store>::DidUpdate::take(), "Parachain heads must be updated once in the block");
		}
	}
}

impl<T: Trait> Module<T> {
	/// Calculate the current block's duty roster using system's random seed.
	pub fn calculate_duty_roster() -> DutyRoster {
		let parachains = Self::active_parachains();
		let parachain_count = parachains.len();
		let validator_count = <session::Module<T>>::validator_count() as usize;
		let validators_per_parachain = if parachain_count != 0 { (validator_count - 1) / parachain_count } else { 0 };

		let mut roles_val = (0..validator_count).map(|i| match i {
			i if i < parachain_count * validators_per_parachain => {
				let idx = i / validators_per_parachain;
				Chain::Parachain(parachains[idx].clone())
			}
			_ => Chain::Relay,
		}).collect::<Vec<_>>();

		let mut roles_gua = roles_val.clone();

		let mut random_seed = system::Module::<T>::random_seed().as_ref().to_vec();
		random_seed.extend(b"validator_role_pairs");
		let mut seed = BlakeTwo256::hash(&random_seed);

		// shuffle
		for i in 0..(validator_count - 1) {
			// 8 bytes of entropy used per cycle, 32 bytes entropy per hash
			let offset = (i * 8 % 32) as usize;

			// number of roles remaining to select from.
			let remaining = (validator_count - i) as usize;

			// 4 * 2 32-bit ints per 256-bit seed.
			let val_index = u32::decode(&mut &seed[offset..offset + 4]).expect("using 4 bytes for a 32-bit quantity") as usize % remaining;
			let gua_index = u32::decode(&mut &seed[offset + 4..offset + 8]).expect("using 4 bytes for a 32-bit quantity") as usize % remaining;

			if offset == 24 {
				// into the last 8 bytes - rehash to gather new entropy
				seed = BlakeTwo256::hash(seed.as_ref());
			}

			// exchange last item with randomly chosen first.
			roles_val.swap(remaining - 1, val_index);
			roles_gua.swap(remaining - 1, gua_index);
		}

		DutyRoster {
			validator_duty: roles_val,
			guarantor_duty: roles_gua,
		}
	}

/*
	// TODO: Consider integrating if needed.
	/// Extract the parachain heads from the block.
	pub fn parachain_heads(&self) -> &[CandidateReceipt] {
		let x = self.inner.extrinsics.get(PARACHAINS_SET_POSITION as usize).and_then(|xt| match xt.function {
			Call::Parachains(ParachainsCall::set_heads(ref x)) => Some(&x[..]),
			_ => None
		});

		match x {
			Some(x) => x,
			None => panic!("Invalid polkadot block asserted at {:?}", self.file_line),
		}
	}
*/
}

impl<T: Trait> ProvideInherent for Module<T> {
	type Inherent = Vec<CandidateReceipt>;
	type Call = Call<T>;
	type Error = RuntimeString;

	fn create_inherent_extrinsics(data: Self::Inherent) -> Vec<(u32, Self::Call)> {
		vec![(T::SET_POSITION, Call::set_heads(data))]
	}

	fn check_inherent<Block: BlockT, F: Fn(&Block::Extrinsic) -> Option<&Self::Call>>(
		block: &Block, _data: Self::Inherent, extract_function: &F
	) -> ::rstd::result::Result<(), Self::Error> {
		let has_heads = block
			.extrinsics()
			.get(T::SET_POSITION as usize)
			.map_or(false, |xt| {
			xt.is_signed() == Some(true) && match extract_function(&xt) {
				Some(Call::set_heads(_)) => true,
				_ => false,
			}
		});

		if !has_heads { return Err("No valid parachains inherent in block".into()) }

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstd::marker::PhantomData;
	use sr_io::{TestExternalities, with_externalities};
	use substrate_primitives::{H256, Blake2Hasher};
	use sr_primitives::BuildStorage;
	use sr_primitives::traits::{Identity, BlakeTwo256};
	use sr_primitives::testing::{Digest, Header, DigestItem};
	use {consensus, timestamp};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	impl consensus::Trait for Test {
		const NOTE_OFFLINE_POSITION: u32 = 1;
		type SessionKey = u64;
		type OnOfflineValidator = ();
		type Log = DigestItem;
	}
	impl system::Trait for Test {
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Digest = Digest;
		type AccountId = u64;
		type Header = Header;
		type Event = ();
		type Log = DigestItem;
	}
	impl session::Trait for Test {
		type ConvertAccountIdToSessionKey = Identity;
		type OnSessionChange = ();
		type Event = ();
	}
	impl timestamp::Trait for Test {
		const TIMESTAMP_SET_POSITION: u32 = 0;
		type Moment = u64;
	}
	impl Trait for Test {
		const SET_POSITION: u32 = 0;
	}

	type Parachains = Module<Test>;

	fn new_test_ext(parachains: Vec<(Id, Vec<u8>, Vec<u8>)>) -> TestExternalities<Blake2Hasher> {
		let mut t = system::GenesisConfig::<Test>::default().build_storage().unwrap().0;
		t.extend(consensus::GenesisConfig::<Test>{
			code: vec![],
			authorities: vec![1, 2, 3],
			_genesis_phantom_data: PhantomData,
		}.build_storage().unwrap().0);
		t.extend(session::GenesisConfig::<Test>{
			session_length: 1000,
			validators: vec![1, 2, 3, 4, 5, 6, 7, 8],
			_genesis_phantom_data: PhantomData,
		}.build_storage().unwrap().0);
		t.extend(GenesisConfig::<Test>{
			parachains: parachains,
			_genesis_phantom_data: PhantomData,
		}.build_storage().unwrap().0);
		t.into()
	}

	#[test]
	fn active_parachains_should_work() {
		let parachains = vec![
			(5u32.into(), vec![1,2,3], vec![1]),
			(100u32.into(), vec![4,5,6], vec![2]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			assert_eq!(Parachains::active_parachains(), vec![5u32.into(), 100u32.into()]);
			assert_eq!(Parachains::parachain_code(&5u32.into()), Some(vec![1,2,3]));
			assert_eq!(Parachains::parachain_code(&100u32.into()), Some(vec![4,5,6]));
		});
	}

	#[test]
	fn register_deregister() {
		let parachains = vec![
			(5u32.into(), vec![1,2,3], vec![1]),
			(100u32.into(), vec![4,5,6], vec![2,]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			assert_eq!(Parachains::active_parachains(), vec![5u32.into(), 100u32.into()]);

			assert_eq!(Parachains::parachain_code(&5u32.into()), Some(vec![1,2,3]));
			assert_eq!(Parachains::parachain_code(&100u32.into()), Some(vec![4,5,6]));

			assert_ok!(Parachains::register_parachain(99u32.into(), vec![7,8,9], vec![1, 1, 1]));

			assert_eq!(Parachains::active_parachains(), vec![5u32.into(), 99u32.into(), 100u32.into()]);
			assert_eq!(Parachains::parachain_code(&99u32.into()), Some(vec![7,8,9]));

			assert_ok!(Parachains::deregister_parachain(5u32.into()));

			assert_eq!(Parachains::active_parachains(), vec![99u32.into(), 100u32.into()]);
			assert_eq!(Parachains::parachain_code(&5u32.into()), None);
		});
	}

	#[test]
	fn duty_roster_works() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			let check_roster = |duty_roster: &DutyRoster| {
				assert_eq!(duty_roster.validator_duty.len(), 8);
				assert_eq!(duty_roster.guarantor_duty.len(), 8);
				for i in (0..2).map(Id::from) {
					assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Parachain(i)).count(), 3);
					assert_eq!(duty_roster.guarantor_duty.iter().filter(|&&j| j == Chain::Parachain(i)).count(), 3);
				}
				assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Relay).count(), 2);
				assert_eq!(duty_roster.guarantor_duty.iter().filter(|&&j| j == Chain::Relay).count(), 2);
			};

			system::Module::<Test>::set_random_seed([0u8; 32].into());
			let duty_roster_0 = Parachains::calculate_duty_roster();
			check_roster(&duty_roster_0);

			system::Module::<Test>::set_random_seed([1u8; 32].into());
			let duty_roster_1 = Parachains::calculate_duty_roster();
			check_roster(&duty_roster_1);
			assert!(duty_roster_0 != duty_roster_1);


			system::Module::<Test>::set_random_seed([2u8; 32].into());
			let duty_roster_2 = Parachains::calculate_duty_roster();
			check_roster(&duty_roster_2);
			assert!(duty_roster_0 != duty_roster_2);
			assert!(duty_roster_1 != duty_roster_2);
		});
	}
}
