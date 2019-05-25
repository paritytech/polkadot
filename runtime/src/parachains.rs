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
use codec::{Decode, HasCompact};

use bitvec::BigEndian;
use sr_primitives::traits::{Hash as HashT, BlakeTwo256, Member};
use primitives::{Hash, parachain::{Id as ParaId, Chain, DutyRoster, AttestedCandidate, Statement, AccountIdConversion}};
use {system, session};

use srml_support::{StorageValue, StorageMap, Parameter, dispatch::Result};
#[cfg(feature = "std")]
use srml_support::storage::hashed::generator;

use inherents::{ProvideInherent, InherentData, RuntimeString, MakeFatalError, InherentIdentifier};

#[cfg(any(feature = "std", test))]
use sr_primitives::{StorageOverlay, ChildrenStorageOverlay};

#[cfg(any(feature = "std", test))]
use rstd::marker::PhantomData;

use system::ensure_none;

/// Parachain registration API.
pub trait ParachainRegistrar<AccountId> {
	/// An identifier for a parachain.
	type ParaId: Member + Parameter + Default + AccountIdConversion<AccountId> + Copy + HasCompact;

	/// Create a new unique parachain identity for later registration.
	fn new_id() -> Self::ParaId;

	/// Register a parachain with given `code` and `initial_head_data`. `id` must not yet be registered or it will
	/// result in a error.
	fn register_parachain(id: Self::ParaId, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result;

	/// Deregister a parachain with given `id`. If `id` is not currently registered, an error is returned.
	fn deregister_parachain(id: Self::ParaId) -> Result;
}

impl<T: Trait> ParachainRegistrar<T::AccountId> for Module<T> {
	type ParaId = ParaId;
	fn new_id() -> ParaId {
		<NextFreeId<T>>::mutate(|n| { let r = *n; *n = ParaId::from(u32::from(*n) + 1); r })
	}
	fn register_parachain(id: ParaId, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result {
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
	fn deregister_parachain(id: ParaId) -> Result {
		let mut parachains = Self::active_parachains();
		match parachains.binary_search(&id) {
			Ok(idx) => { parachains.remove(idx); }
			Err(_) => return Ok(()),
		}

		<Code<T>>::remove(id);
		<Heads<T>>::remove(id);

		// clear all routing entries to and from other parachains.
		for other in parachains.iter().cloned() {
			<Routing<T>>::remove((id, other));
			<Routing<T>>::remove((other, id));
		}

		<Parachains<T>>::put(parachains);

		Ok(())
	}
}

pub trait Trait: session::Trait {
}

// result of <NodeCodec<Blake2Hasher> as trie_db::NodeCodec<Blake2Hasher>>::hashed_null_node()
const EMPTY_TRIE_ROOT: [u8; 32] = [
	3, 23, 10, 46, 117, 151, 183, 183, 227, 216, 76, 5, 57, 29, 19, 154,
	98, 177, 87, 231, 135, 134, 216, 192, 130, 242, 157, 207, 76, 17, 19, 20
];

decl_storage! {
	trait Store for Module<T: Trait> as Parachains {
		// Vector of all parachain IDs.
		pub Parachains get(active_parachains): Vec<ParaId>;
		// The parachains registered at present.
		pub Code get(parachain_code): map ParaId => Option<Vec<u8>>;
		// The heads of the parachains registered at present.
		pub Heads get(parachain_head): map ParaId => Option<Vec<u8>>;
		// message routing roots (from, to).
		pub Routing: map (ParaId, ParaId) => Option<Hash>;

		// Did the parachain heads get updated in this block?
		DidUpdate: bool;

		/// The next unused ParaId value.
		NextFreeId: ParaId;
	}
	add_extra_genesis {
		config(parachains): Vec<(ParaId, Vec<u8>, Vec<u8>)>;
		config(_phdata): PhantomData<T>;
		build(|storage: &mut StorageOverlay, _: &mut ChildrenStorageOverlay, config: &GenesisConfig<T>| {
			let mut p = config.parachains.clone();
			p.sort_unstable_by_key(|&(ref id, _, _)| id.clone());
			p.dedup_by_key(|&mut (ref id, _, _)| id.clone());

			let only_ids: Vec<_> = p.iter().map(|&(ref id, _, _)| id).cloned().collect();

			<Parachains<T> as generator::StorageValue<_>>::put(&only_ids, storage);

			for (id, code, genesis) in p {
				// no ingress -- a chain cannot be routed to until it is live.
				<Code<T> as generator::StorageMap<_, _>>::insert(&id, &code, storage);
				<Heads<T> as generator::StorageMap<_, _>>::insert(&id, &genesis, storage);
			}
		});
	}
}

decl_module! {
	/// Parachains module.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		/// Provide candidate receipts for parachains, in ascending order by id.
		fn set_heads(origin, heads: Vec<AttestedCandidate>) -> Result {
			ensure_none(origin)?;
			ensure!(!<DidUpdate<T>>::exists(), "Parachain heads must be updated only once in the block");

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
						last_id.as_ref().map_or(true, |x| x < &head.parachain_index()),
						"Parachain candidates out of order by ID"
					);

					// must be unknown since active parachains are always sorted.
					ensure!(
						iter.find(|x| x == &&head.parachain_index()).is_some(),
						"Submitted candidate for unregistered or out-of-order parachain {}"
					);

					Self::check_egress_queue_roots(&head, &active_parachains)?;

					last_id = Some(head.parachain_index());
				}
			}

			Self::check_attestations(&heads)?;

			for head in heads {
				let id = head.parachain_index();
				<Heads<T>>::insert(id, head.candidate.head_data.0);

				// update egress.
				for &(to, root) in &head.candidate.egress_queue_roots {
					<Routing<T>>::insert((id, to), root);
				}
			}

			<DidUpdate<T>>::put(true);

			Ok(())
		}

		/// Register a parachain with given code.
		/// Fails if given ID is already used.
		pub fn register_parachain(id: ParaId, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result {
			<Self as ParachainRegistrar<T::AccountId>>::register_parachain(id, code, initial_head_data)
		}

		/// Deregister a parachain with given id
		pub fn deregister_parachain(id: ParaId) -> Result {
			<Self as ParachainRegistrar<T::AccountId>>::deregister_parachain(id)
		}

		fn on_finalize(_n: T::BlockNumber) {
			assert!(<Self as Store>::DidUpdate::take(), "Parachain heads must be updated once in the block");
		}
	}
}

fn majority_of(list_len: usize) -> usize {
	list_len / 2 + list_len % 2
}

fn localized_payload(statement: Statement, parent_hash: ::primitives::Hash) -> Vec<u8> {
	use codec::Encode;

	let mut encoded = statement.encode();
	encoded.extend(parent_hash.as_ref());
	encoded
}

impl<T: Trait> Module<T> {
	/// Calculate the current block's duty roster using system's random seed.
	pub fn calculate_duty_roster() -> DutyRoster {
		let parachains = Self::active_parachains();
		let parachain_count = parachains.len();
		let validator_count = <consensus::Module<T>>::authorities().len();
		let validators_per_parachain = if parachain_count != 0 { (validator_count - 1) / parachain_count } else { 0 };

		let mut roles_val = (0..validator_count).map(|i| match i {
			i if i < parachain_count * validators_per_parachain => {
				let idx = i / validators_per_parachain;
				Chain::Parachain(parachains[idx].clone())
			}
			_ => Chain::Relay,
		}).collect::<Vec<_>>();


		let mut seed = {
			let phrase = b"validator_role_pairs";
			let seed = system::Module::<T>::random(&phrase[..]);
			let seed_len = seed.as_ref().len();
			let needed_bytes = validator_count * 4;

			// hash only the needed bits of the random seed.
			// if earlier bits are influencable, they will not factor into
			// the seed used here.
			let seed_off = if needed_bytes >= seed_len {
				0
			} else {
				seed_len - needed_bytes
			};

			BlakeTwo256::hash(&seed.as_ref()[seed_off..])
		};

		// shuffle
		for i in 0..(validator_count - 1) {
			// 4 bytes of entropy used per cycle, 32 bytes entropy per hash
			let offset = (i * 4 % 32) as usize;

			// number of roles remaining to select from.
			let remaining = (validator_count - i) as usize;

			// 8 32-bit ints per 256-bit seed.
			let val_index = u32::decode(&mut &seed[offset..offset + 4]).expect("using 4 bytes for a 32-bit quantity") as usize % remaining;

			if offset == 28 {
				// into the last 4 bytes - rehash to gather new entropy
				seed = BlakeTwo256::hash(seed.as_ref());
			}

			// exchange last item with randomly chosen first.
			roles_val.swap(remaining - 1, val_index);
		}

		DutyRoster {
			validator_duty: roles_val,
		}
	}

	/// Calculate the ingress to a specific parachain.
	///
	/// Yields a list of parachains being routed from, and the egress
	/// queue roots to consider.
	pub fn ingress(to: ParaId) -> Option<Vec<(ParaId, Hash)>> {
		let active_parachains = Self::active_parachains();
		if !active_parachains.contains(&to) { return None }

		Some(active_parachains.into_iter().filter(|i| i != &to)
			.filter_map(move |from| {
				<Routing<T>>::get((from, to.clone())).map(move |h| (from, h))
			})
			.collect())
	}

	fn check_egress_queue_roots(head: &AttestedCandidate, active_parachains: &[ParaId]) -> Result {
		let mut last_egress_id = None;
		let mut iter = active_parachains.iter();
		for (egress_para_id, root) in &head.candidate.egress_queue_roots {
			// egress routes should be ascending order by parachain ID without duplicate.
			ensure!(
				last_egress_id.as_ref().map_or(true, |x| x < &egress_para_id),
				"Egress routes out of order by ID"
			);

			// a parachain can't route to self
			ensure!(
				*egress_para_id != head.candidate.parachain_index,
				"Parachain routing to self"
			);

			// no empty trie roots
			ensure!(
				*root != EMPTY_TRIE_ROOT.into(),
				"Empty trie root included"
			);

			// can't route to a parachain which doesn't exist
			ensure!(
				iter.find(|x| x == &egress_para_id).is_some(),
				"Routing to non-existent parachain"
			);

			last_egress_id = Some(egress_para_id)
		}
		Ok(())
	}

	// check the attestations on these candidates. The candidates should have been checked
	// that each candidates' chain ID is valid.
	fn check_attestations(attested_candidates: &[AttestedCandidate]) -> Result {
		use primitives::parachain::ValidityAttestation;
		use sr_primitives::traits::Verify;

		// returns groups of slices that have the same chain ID.
		// assumes the inner slice is sorted by id.
		struct GroupedDutyIter<'a> {
			next_idx: usize,
			inner: &'a [(usize, ParaId)],
		}

		impl<'a> GroupedDutyIter<'a> {
			fn new(inner: &'a [(usize, ParaId)]) -> Self {
				GroupedDutyIter { next_idx: 0, inner }
			}

			fn group_for(&mut self, wanted_id: ParaId) -> Option<&'a [(usize, ParaId)]> {
				while let Some((id, keys)) = self.next() {
					if wanted_id == id {
						return Some(keys)
					}
				}

				None
			}
		}

		impl<'a> Iterator for GroupedDutyIter<'a> {
			type Item = (ParaId, &'a [(usize, ParaId)]);

			fn next(&mut self) -> Option<Self::Item> {
				if self.next_idx == self.inner.len() { return None }
				let start_idx = self.next_idx;
				self.next_idx += 1;
				let start_id = self.inner[start_idx].1;

				while self.inner.get(self.next_idx).map_or(false, |&(_, ref id)| id == &start_id) {
					self.next_idx += 1;
				}

				Some((start_id, &self.inner[start_idx..self.next_idx]))
			}
		}

		let authorities = super::Consensus::authorities();
		let duty_roster = Self::calculate_duty_roster();

		// convert a duty roster, which is originally a Vec<Chain>, where each
		// item corresponds to the same position in the session keys, into
		// a list containing (index, parachain duty) where indices are into the session keys.
		// this list is sorted ascending by parachain duty, just like the
		// parachain candidates are.
		let make_sorted_duties = |duty: &[Chain]| {
			let mut sorted_duties = Vec::with_capacity(duty.len());
			for (val_idx, duty) in duty.iter().enumerate() {
				let id = match duty {
					Chain::Relay => continue,
					Chain::Parachain(id) => id,
				};

				let idx = sorted_duties.binary_search_by_key(&id, |&(_, ref id)| id)
					.unwrap_or_else(|idx| idx);

				sorted_duties.insert(idx, (val_idx, *id));
			}

			sorted_duties
		};

		let sorted_validators = make_sorted_duties(&duty_roster.validator_duty);

		let parent_hash = super::System::parent_hash();
		let localized_payload = |statement: Statement| localized_payload(statement, parent_hash);

		let mut validator_groups = GroupedDutyIter::new(&sorted_validators[..]);

		for candidate in attested_candidates {
			let validator_group = validator_groups.group_for(candidate.parachain_index())
				.ok_or("no validator group for parachain")?;

			ensure!(
				candidate.validity_votes.len() >= majority_of(validator_group.len()),
				"Not enough validity attestations"
			);

			let mut candidate_hash = None;
			let mut encoded_implicit = None;
			let mut encoded_explicit = None;

			// track which voters have voted already, 1 bit per authority.
			let mut track_voters = bitvec![0; authorities.len()];
			for (auth_index, validity_attestation) in &candidate.validity_votes {
				let auth_index = *auth_index as usize;
				// protect against double-votes.
				match validator_group.iter().find(|&(idx, _)| *idx == auth_index) {
					None => return Err("Attesting validator not on this chain's validation duty."),
					Some(&(idx, _)) => {
						if track_voters.get(idx) {
							return Err("Voter already attested validity once")
						}
						track_voters.set(idx, true)
					}
				}

				let (payload, sig) = match validity_attestation {
					ValidityAttestation::Implicit(sig) => {
						let payload = encoded_implicit.get_or_insert_with(|| localized_payload(
							Statement::Candidate(candidate.candidate.clone()),
						));

						(payload, sig)
					}
					ValidityAttestation::Explicit(sig) => {
						let hash = candidate_hash
							.get_or_insert_with(|| candidate.candidate.hash())
							.clone();

						let payload = encoded_explicit.get_or_insert_with(|| localized_payload(
							Statement::Valid(hash),
						));

						(payload, sig)
					}
				};

				ensure!(
					sig.verify(&payload[..], &authorities[auth_index]),
					"Candidate validity attestation signature is bad."
				);
			}
		}

		Ok(())
	}

/*
	// TODO: Consider integrating if needed. (https://github.com/paritytech/polkadot/issues/223)
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

pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"newheads";

pub type InherentType = Vec<AttestedCandidate>;

impl<T: Trait> ProvideInherent for Module<T> {
	type Call = Call<T>;
	type Error = MakeFatalError<RuntimeString>;
	const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

	fn create_inherent(data: &InherentData) -> Option<Self::Call> {
		let data = data.get_data::<InherentType>(&INHERENT_IDENTIFIER)
			.expect("Parachain heads could not be decoded.")
			.expect("No parachain heads found in inherent data.");

		Some(Call::set_heads(data))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sr_io::{TestExternalities, with_externalities};
	use substrate_primitives::{H256, Blake2Hasher};
	use substrate_trie::NodeCodec;
	use sr_primitives::{generic, BuildStorage};
	use sr_primitives::traits::{BlakeTwo256, IdentityLookup};
	use primitives::{parachain::{CandidateReceipt, HeadData, ValidityAttestation, ValidatorIndex}, SessionKey};
	use keyring::{AuthorityKeyring, AccountKeyring};
	use {consensus, timestamp};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	impl consensus::Trait for Test {
		type InherentOfflineReport = ();
		type SessionKey = SessionKey;
		type Log = ::Log;
	}
	impl system::Trait for Test {
		type Origin = Origin;
		type Index = ::Nonce;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Digest = generic::Digest<::Log>;
		type AccountId = ::AccountId;
		type Lookup = IdentityLookup<::AccountId>;
		type Header = ::Header;
		type Event = ();
		type Log = ::Log;
	}
	impl session::Trait for Test {
		type ConvertAccountIdToSessionKey = ();
		type OnSessionChange = ();
		type Event = ();
	}
	impl timestamp::Trait for Test {
		type Moment = u64;
		type OnTimestampSet = ();
	}
	impl Trait for Test {}

	type Parachains = Module<Test>;
	type System = system::Module<Test>;

	fn new_test_ext(parachains: Vec<(ParaId, Vec<u8>, Vec<u8>)>) -> TestExternalities<Blake2Hasher> {
		let mut t = system::GenesisConfig::<Test>::default().build_storage().unwrap().0;
		let authority_keys = [
			AuthorityKeyring::Alice,
			AuthorityKeyring::Bob,
			AuthorityKeyring::Charlie,
			AuthorityKeyring::Dave,
			AuthorityKeyring::Eve,
			AuthorityKeyring::Ferdie,
			AuthorityKeyring::One,
			AuthorityKeyring::Two,
		];
		let validator_keys = [
			AccountKeyring::Alice,
			AccountKeyring::Bob,
			AccountKeyring::Charlie,
			AccountKeyring::Dave,
			AccountKeyring::Eve,
			AccountKeyring::Ferdie,
			AccountKeyring::One,
			AccountKeyring::Two,
		];

		t.extend(consensus::GenesisConfig::<Test>{
			code: vec![],
			authorities: authority_keys.iter().map(|k| SessionKey::from(*k)).collect(),
		}.build_storage().unwrap().0);
		t.extend(session::GenesisConfig::<Test>{
			session_length: 1000,
			validators: validator_keys.iter().map(|k| ::AccountId::from(*k)).collect(),
			keys: vec![],
		}.build_storage().unwrap().0);
		t.extend(GenesisConfig::<Test>{
			parachains: parachains,
			_phdata: Default::default(),
		}.build_storage().unwrap().0);
		t.into()
	}

	fn make_attestations(candidate: &mut AttestedCandidate) {
		let mut vote_implicit = false;
		let parent_hash = ::System::parent_hash();

		let duty_roster = Parachains::calculate_duty_roster();
		let candidate_hash = candidate.candidate.hash();

		let authorities = ::Consensus::authorities();
		let extract_key = |public: SessionKey| {
			AuthorityKeyring::from_raw_public(public.0).unwrap()
		};

		let validation_entries = duty_roster.validator_duty.iter()
			.enumerate();

		for (idx, &duty) in validation_entries {
			if duty != Chain::Parachain(candidate.parachain_index()) { continue }
			vote_implicit = !vote_implicit;

			let key = extract_key(authorities[idx].clone());

			let statement = if vote_implicit {
				Statement::Candidate(candidate.candidate.clone())
			} else {
				Statement::Valid(candidate_hash.clone())
			};

			let payload = localized_payload(statement, parent_hash);
			let signature = key.sign(&payload[..]).into();

			candidate.validity_votes.push((idx as ValidatorIndex, if vote_implicit {
				ValidityAttestation::Implicit(signature)
			} else {
				ValidityAttestation::Explicit(signature)
			}));
		}
	}

	fn new_candidate_with_egress_roots(egress_queue_roots: Vec<(ParaId, H256)>) -> AttestedCandidate {
		AttestedCandidate {
			validity_votes: vec![],
			candidate: CandidateReceipt {
				parachain_index: 0.into(),
				collator: Default::default(),
				signature: Default::default(),
				head_data: HeadData(vec![1, 2, 3]),
				balance_uploads: vec![],
				egress_queue_roots,
				fees: 0,
				block_data_hash: Default::default(),
			}
		}
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
				for i in (0..2).map(ParaId::from) {
					assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Parachain(i)).count(), 3);
				}
				assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Relay).count(), 2);
			};

			let duty_roster_0 = Parachains::calculate_duty_roster();
			check_roster(&duty_roster_0);

			System::initialize(&1, &H256::from([1; 32]), &Default::default());
			let duty_roster_1 = Parachains::calculate_duty_roster();
			check_roster(&duty_roster_1);
			assert!(duty_roster_0 != duty_roster_1);


			System::initialize(&2, &H256::from([2; 32]), &Default::default());
			let duty_roster_2 = Parachains::calculate_duty_roster();
			check_roster(&duty_roster_2);
			assert!(duty_roster_0 != duty_roster_2);
			assert!(duty_roster_1 != duty_roster_2);
		});
	}

	#[test]
	fn unattested_candidate_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			let candidate = AttestedCandidate {
				validity_votes: vec![],
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					balance_uploads: vec![],
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
				},

			};

			assert!(Parachains::dispatch(Call::set_heads(vec![candidate]), Origin::NONE).is_err());
		})
	}

	#[test]
	fn attested_candidates_accepted_in_order() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			let mut candidate_a = AttestedCandidate {
				validity_votes: vec![],
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					balance_uploads: vec![],
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
				}
			};

			let mut candidate_b = AttestedCandidate {
				validity_votes: vec![],
				candidate: CandidateReceipt {
					parachain_index: 1.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![2, 3, 4]),
					balance_uploads: vec![],
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
				}
			};

			make_attestations(&mut candidate_a);
			make_attestations(&mut candidate_b);

			assert!(Parachains::dispatch(
				Call::set_heads(vec![candidate_b.clone(), candidate_a.clone()]),
				Origin::NONE,
			).is_err());

			assert!(Parachains::dispatch(
				Call::set_heads(vec![candidate_a.clone(), candidate_b.clone()]),
				Origin::NONE,
			).is_ok());
		});
	}

	#[test]
	fn duplicate_vote_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			let mut candidate = AttestedCandidate {
				validity_votes: vec![],
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					balance_uploads: vec![],
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
				}
			};

			make_attestations(&mut candidate);

			let mut double_validity = candidate.clone();
			double_validity.validity_votes.push(candidate.validity_votes[0].clone());

			assert!(Parachains::dispatch(
				Call::set_heads(vec![double_validity]),
				Origin::NONE,
			).is_err());
		});
	}

	#[test]
	fn ingress_works() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
			(99u32.into(), vec![1, 2, 3], vec![4, 5, 6]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			let from_a = vec![(1.into(), [1; 32].into())];
			let mut candidate_a = AttestedCandidate {
				validity_votes: vec![],
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					balance_uploads: vec![],
					egress_queue_roots: from_a.clone(),
					fees: 0,
					block_data_hash: Default::default(),
				}
			};

			let from_b = vec![(99.into(), [1; 32].into())];
			let mut candidate_b = AttestedCandidate {
				validity_votes: vec![],
				candidate: CandidateReceipt {
					parachain_index: 1.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					balance_uploads: vec![],
					egress_queue_roots: from_b.clone(),
					fees: 0,
					block_data_hash: Default::default(),
				}
			};

			make_attestations(&mut candidate_a);
			make_attestations(&mut candidate_b);

			assert_eq!(Parachains::ingress(ParaId::from(1)), Some(Vec::new()));
			assert_eq!(Parachains::ingress(ParaId::from(99)), Some(Vec::new()));

			assert!(Parachains::dispatch(
				Call::set_heads(vec![candidate_a, candidate_b]),
				Origin::NONE,
			).is_ok());

			assert_eq!(
				Parachains::ingress(ParaId::from(1)),
				Some(vec![(0.into(), [1; 32].into())]),
			);

			assert_eq!(
				Parachains::ingress(ParaId::from(99)),
				Some(vec![(1.into(), [1; 32].into())]),
			);

			assert_ok!(Parachains::deregister_parachain(1u32.into()));

			// after deregistering, there is no ingress to 1 and we stop routing
			// from 1.
			assert_eq!(Parachains::ingress(ParaId::from(1)), None);
			assert_eq!(Parachains::ingress(ParaId::from(99)), Some(Vec::new()));
		});
	}

	#[test]
	fn egress_routed_to_non_existent_parachain_is_rejected() {
		// That no parachain is routed to which doesn't exist
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			// parachain 99 does not exist
			let non_existent = vec![(99.into(), [1; 32].into())];
			let mut candidate = new_candidate_with_egress_roots(non_existent);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				Call::set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Routing to non-existent parachain"), result);
		});
	}

	#[test]
	fn egress_routed_to_self_is_rejected() {
		// That the parachain doesn't route to self
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			// parachain 0 is self
			let to_self = vec![(0.into(), [1; 32].into())];
			let mut candidate = new_candidate_with_egress_roots(to_self);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				Call::set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Parachain routing to self"), result);
		});
	}

	#[test]
	fn egress_queue_roots_out_of_order_rejected() {
		// That the list of egress queue roots is in ascending order by `ParaId`.
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			// parachain 0 is self
			let out_of_order = vec![(1.into(), [1; 32].into()), ((0.into(), [1; 32].into()))];
			let mut candidate = new_candidate_with_egress_roots(out_of_order);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				Call::set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Egress routes out of order by ID"), result);
		});
	}

	#[test]
	fn egress_queue_roots_empty_trie_roots_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
			(2u32.into(), vec![], vec![]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			// parachain 0 is self
			let contains_empty_trie_root = vec![(1.into(), [1; 32].into()), ((2.into(), EMPTY_TRIE_ROOT.into()))];
			let mut candidate = new_candidate_with_egress_roots(contains_empty_trie_root);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				Call::set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Empty trie root included"), result);
		});
	}

	#[test]
	fn empty_trie_root_const_is_blake2_hashed_null_node() {
		let hashed_null_node =  <NodeCodec<Blake2Hasher> as trie_db::NodeCodec<Blake2Hasher>>::hashed_null_node();
		assert_eq!(hashed_null_node, EMPTY_TRIE_ROOT.into())
	}
}
