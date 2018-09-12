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

use runtime_primitives::traits::{Hash, BlakeTwo256, OnFinalise};
use primitives::parachain::{Id, Chain, DutyRoster, CandidateReceipt};
use {system, session};

use srml_support::{StorageValue, StorageMap};
use srml_support::dispatch::Result;

#[cfg(any(feature = "std", test))]
use rstd::marker::PhantomData;

#[cfg(any(feature = "std", test))]
use runtime_primitives;

#[cfg(any(feature = "std", test))]
use std::collections::HashMap;

use system::{ensure_root, ensure_inherent};

pub trait Trait: system::Trait<Hash = ::primitives::Hash> + session::Trait {
	/// The position of the set_heads call in the block.
	const SET_POSITION: u32;
}

decl_module! {
	/// Parachains module.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		/// Provide candidate receipts for parachains, in ascending order by id.
		fn set_heads(origin, heads: Vec<CandidateReceipt>) -> Result;

		fn register_parachain(origin, id: Id, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result;
		fn deregister_parachain(origin, id: Id) -> Result;
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as Parachains {
		// Vector of all parachain IDs.
		pub Parachains get(active_parachains): default Vec<Id>;
		// The parachains registered at present.
		pub Code get(parachain_code): map [ Id => Vec<u8> ];
		// The heads of the parachains registered at present. these are kept sorted.
		pub Heads get(parachain_head): map [ Id => Vec<u8> ];

		// Did the parachain heads get updated in this block?
		DidUpdate: default bool;
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

		let mut random_seed = system::Module::<T>::random_seed().to_vec();
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
				seed = BlakeTwo256::hash(&seed);
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

	/// Register a parachain with given code.
	/// Fails if given ID is already used.
	pub fn register_parachain(origin: T::Origin, id: Id, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result {
		ensure_root(origin)?;
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
	pub fn deregister_parachain(origin: T::Origin, id: Id) -> Result {
		ensure_root(origin)?;
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

	fn set_heads(origin: T::Origin, heads: Vec<CandidateReceipt>) -> Result {
		ensure_inherent(origin)?;
		ensure!(!<DidUpdate<T>>::exists(), "Parachain heads must be updated only once in the block");
		ensure!(
			<system::Module<T>>::extrinsic_index() == Some(T::SET_POSITION),
			"Parachain heads update extrinsic must be at position {} in the block"
//			, T::SET_POSITION
		);

		let active_parachains = Self::active_parachains();
		let mut iter = active_parachains.iter();

		// perform this check before writing to storage.
		for head in &heads {
			ensure!(
				iter.find(|&p| p == &head.parachain_index).is_some(),
				"Submitted candidate for unregistered or out-of-order parachain {}"
//				, head.parachain_index.into_inner()
			);
		}

		for head in heads {
			let id = head.parachain_index.clone();
			<Heads<T>>::insert(id, head.head_data.0);
		}

		<DidUpdate<T>>::put(true);

		Ok(())
	}
}

impl<T: Trait> OnFinalise<T::BlockNumber> for Module<T> {
	fn on_finalise(_n: T::BlockNumber) {
		assert!(<Self as Store>::DidUpdate::take(), "Parachain heads must be updated once in the block");
	}
}

/// Parachains module genesis configuration.
#[cfg(any(feature = "std", test))]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct GenesisConfig<T: Trait> {
	/// The initial parachains, mapped to code and initial head data
	pub parachains: Vec<(Id, Vec<u8>, Vec<u8>)>,
	/// Phantom data.
	#[serde(skip)]
	pub phantom: PhantomData<T>,
}

#[cfg(any(feature = "std", test))]
impl<T: Trait> Default for GenesisConfig<T> {
	fn default() -> Self {
		GenesisConfig {
			parachains: Vec::new(),
			phantom: PhantomData,
		}
	}
}

#[cfg(any(feature = "std", test))]
impl<T: Trait> runtime_primitives::BuildStorage for GenesisConfig<T>
{
	fn build_storage(mut self) -> ::std::result::Result<HashMap<Vec<u8>, Vec<u8>>, String> {
		use codec::Encode;

		self.parachains.sort_unstable_by_key(|&(ref id, _, _)| id.clone());
		self.parachains.dedup_by_key(|&mut (ref id, _, _)| id.clone());

		let only_ids: Vec<_> = self.parachains.iter().map(|&(ref id, _, _)| id).cloned().collect();

		let mut map: HashMap<_, _> = map![
			Self::hash(<Parachains<T>>::key()).to_vec() => only_ids.encode()
		];

		for (id, code, genesis) in self.parachains {
			let code_key = Self::hash(&<Code<T>>::key_for(&id)).to_vec();
			let head_key = Self::hash(&<Heads<T>>::key_for(&id)).to_vec();

			map.insert(code_key, code.encode());
			map.insert(head_key, genesis.encode());
		}

		Ok(map)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use runtime_io::{TestExternalities, with_externalities};
	use substrate_primitives::{H256, Blake2Hasher};
	use runtime_primitives::BuildStorage;
	use runtime_primitives::traits::{Identity, BlakeTwo256};
	use runtime_primitives::testing::{Digest, Header};
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
		type Log = u64;
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
		let mut t = system::GenesisConfig::<Test>::default().build_storage().unwrap();
		t.extend(consensus::GenesisConfig::<Test>{
			code: vec![],
			authorities: vec![1, 2, 3],
		}.build_storage().unwrap());
		t.extend(session::GenesisConfig::<Test>{
			session_length: 1000,
			validators: vec![1, 2, 3, 4, 5, 6, 7, 8],
		}.build_storage().unwrap());
		t.extend(GenesisConfig::<Test>{
			parachains: parachains,
			phantom: PhantomData,
		}.build_storage().unwrap());
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

			assert_ok!(Parachains::register_parachain(Origin::ROOT, 99u32.into(), vec![7,8,9], vec![1, 1, 1]));

			assert_eq!(Parachains::active_parachains(), vec![5u32.into(), 99u32.into(), 100u32.into()]);
			assert_eq!(Parachains::parachain_code(&99u32.into()), Some(vec![7,8,9]));

			assert_ok!(Parachains::deregister_parachain(Origin::ROOT, 5u32.into()));

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
