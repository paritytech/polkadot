// Copyright (C) Parity Technologies (UK) Ltd.
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
use crate::{
	configuration::HostConfiguration,
	initializer::SessionChangeNotification,
	mock::{
		new_test_ext, Configuration, MockGenesisConfig, ParaInclusion, Paras, ParasShared,
		Scheduler, System, Test,
	},
	paras::{ParaGenesisArgs, ParaKind},
	paras_inherent::DisputedBitfield,
	shared::AllowedRelayParentsTracker,
};
use primitives::{SignedAvailabilityBitfields, UncheckedSignedAvailabilityBitfields};

use assert_matches::assert_matches;
use frame_support::assert_noop;
use keyring::Sr25519Keyring;
use parity_scale_codec::DecodeAll;
use primitives::{
	v5::{Assignment, ParasEntry},
	BlockNumber, CandidateCommitments, CandidateDescriptor, CollatorId,
	CompactStatement as Statement, Hash, SignedAvailabilityBitfield, SignedStatement,
	ValidationCode, ValidatorId, ValidityAttestation, PARACHAIN_KEY_TYPE_ID,
};
use sc_keystore::LocalKeystore;
use sp_keystore::{Keystore, KeystorePtr};
use std::sync::Arc;
use test_helpers::{dummy_collator, dummy_collator_signature, dummy_validation_code};

fn default_config() -> HostConfiguration<BlockNumber> {
	let mut config = HostConfiguration::default();
	config.on_demand_cores = 1;
	config.max_code_size = 0b100000;
	config.max_head_data_size = 0b100000;
	config.group_rotation_frequency = u32::MAX;
	config
}

pub(crate) fn genesis_config(paras: Vec<(ParaId, ParaKind)>) -> MockGenesisConfig {
	MockGenesisConfig {
		paras: paras::GenesisConfig {
			paras: paras
				.into_iter()
				.map(|(id, para_kind)| {
					(
						id,
						ParaGenesisArgs {
							genesis_head: Vec::new().into(),
							validation_code: dummy_validation_code(),
							para_kind,
						},
					)
				})
				.collect(),
			..Default::default()
		},
		configuration: configuration::GenesisConfig { config: default_config() },
		..Default::default()
	}
}

fn default_allowed_relay_parent_tracker() -> AllowedRelayParentsTracker<Hash, BlockNumber> {
	let mut allowed = AllowedRelayParentsTracker::default();

	let relay_parent = System::parent_hash();
	let parent_number = System::block_number().saturating_sub(1);

	allowed.update(relay_parent, Hash::zero(), parent_number, 1);
	allowed
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BackingKind {
	#[allow(unused)]
	Unanimous,
	Threshold,
	Lacking,
}

pub(crate) fn collator_sign_candidate(
	collator: Sr25519Keyring,
	candidate: &mut CommittedCandidateReceipt,
) {
	candidate.descriptor.collator = collator.public().into();

	let payload = primitives::collator_signature_payload(
		&candidate.descriptor.relay_parent,
		&candidate.descriptor.para_id,
		&candidate.descriptor.persisted_validation_data_hash,
		&candidate.descriptor.pov_hash,
		&candidate.descriptor.validation_code_hash,
	);

	candidate.descriptor.signature = collator.sign(&payload[..]).into();
	assert!(candidate.descriptor().check_collator_signature().is_ok());
}

pub(crate) fn back_candidate(
	candidate: CommittedCandidateReceipt,
	validators: &[Sr25519Keyring],
	group: &[ValidatorIndex],
	keystore: &KeystorePtr,
	signing_context: &SigningContext,
	kind: BackingKind,
) -> BackedCandidate {
	let mut validator_indices = bitvec::bitvec![u8, BitOrderLsb0; 0; group.len()];
	let threshold = minimum_backing_votes(group.len());

	let signing = match kind {
		BackingKind::Unanimous => group.len(),
		BackingKind::Threshold => threshold,
		BackingKind::Lacking => threshold.saturating_sub(1),
	};

	let mut validity_votes = Vec::with_capacity(signing);
	let candidate_hash = candidate.hash();

	for (idx_in_group, val_idx) in group.iter().enumerate().take(signing) {
		let key: Sr25519Keyring = validators[val_idx.0 as usize];
		*validator_indices.get_mut(idx_in_group).unwrap() = true;

		let signature = SignedStatement::sign(
			&keystore,
			Statement::Valid(candidate_hash),
			signing_context,
			*val_idx,
			&key.public().into(),
		)
		.unwrap()
		.unwrap()
		.signature()
		.clone();

		validity_votes.push(ValidityAttestation::Explicit(signature).into());
	}

	let backed = BackedCandidate { candidate, validity_votes, validator_indices };

	let successfully_backed =
		primitives::check_candidate_backing(&backed, signing_context, group.len(), |i| {
			Some(validators[group[i].0 as usize].public().into())
		})
		.ok()
		.unwrap_or(0) >=
			threshold;

	match kind {
		BackingKind::Unanimous | BackingKind::Threshold => assert!(successfully_backed),
		BackingKind::Lacking => assert!(!successfully_backed),
	};

	backed
}

pub(crate) fn run_to_block_default_notifications(to: BlockNumber, new_session: Vec<BlockNumber>) {
	run_to_block(to, |b| {
		new_session.contains(&b).then_some(SessionChangeNotification {
			prev_config: Configuration::config(),
			new_config: Configuration::config(),
			session_index: ParasShared::session_index() + 1,
			..Default::default()
		})
	});
}

pub(crate) fn run_to_block(
	to: BlockNumber,
	new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
) {
	while System::block_number() < to {
		let b = System::block_number();

		ParaInclusion::initializer_finalize();
		Paras::initializer_finalize(b);
		ParasShared::initializer_finalize();

		if let Some(notification) = new_session(b + 1) {
			ParasShared::initializer_on_new_session(
				notification.session_index,
				notification.random_seed,
				&notification.new_config,
				notification.validators.clone(),
			);
			let outgoing = Paras::initializer_on_new_session(&notification);
			ParaInclusion::initializer_on_new_session(&notification, &outgoing);
		}

		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		ParasShared::initializer_initialize(b + 1);
		Paras::initializer_initialize(b + 1);
		ParaInclusion::initializer_initialize(b + 1);
	}
}

pub(crate) fn expected_bits() -> usize {
	Paras::parachains().len() + Configuration::config().on_demand_cores as usize
}

fn default_bitfield() -> AvailabilityBitfield {
	AvailabilityBitfield(bitvec::bitvec![u8, BitOrderLsb0; 0; expected_bits()])
}

fn default_availability_votes() -> BitVec<u8, BitOrderLsb0> {
	bitvec::bitvec![u8, BitOrderLsb0; 0; ParasShared::active_validator_keys().len()]
}

fn default_backing_bitfield() -> BitVec<u8, BitOrderLsb0> {
	bitvec::bitvec![u8, BitOrderLsb0; 0; ParasShared::active_validator_keys().len()]
}

fn backing_bitfield(v: &[usize]) -> BitVec<u8, BitOrderLsb0> {
	let mut b = default_backing_bitfield();
	for i in v {
		b.set(*i, true);
	}
	b
}

pub(crate) fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

pub(crate) fn sign_bitfield(
	keystore: &KeystorePtr,
	key: &Sr25519Keyring,
	validator_index: ValidatorIndex,
	bitfield: AvailabilityBitfield,
	signing_context: &SigningContext,
) -> SignedAvailabilityBitfield {
	SignedAvailabilityBitfield::sign(
		&keystore,
		bitfield,
		&signing_context,
		validator_index,
		&key.public().into(),
	)
	.unwrap()
	.unwrap()
}

pub(crate) struct TestCandidateBuilder {
	pub(crate) para_id: ParaId,
	pub(crate) head_data: HeadData,
	pub(crate) para_head_hash: Option<Hash>,
	pub(crate) pov_hash: Hash,
	pub(crate) relay_parent: Hash,
	pub(crate) persisted_validation_data_hash: Hash,
	pub(crate) new_validation_code: Option<ValidationCode>,
	pub(crate) validation_code: ValidationCode,
	pub(crate) hrmp_watermark: BlockNumber,
}

impl std::default::Default for TestCandidateBuilder {
	fn default() -> Self {
		let zeros = Hash::zero();
		Self {
			para_id: 0.into(),
			head_data: Default::default(),
			para_head_hash: None,
			pov_hash: zeros,
			relay_parent: zeros,
			persisted_validation_data_hash: zeros,
			new_validation_code: None,
			validation_code: dummy_validation_code(),
			hrmp_watermark: 0u32.into(),
		}
	}
}

impl TestCandidateBuilder {
	pub(crate) fn build(self) -> CommittedCandidateReceipt {
		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				persisted_validation_data_hash: self.persisted_validation_data_hash,
				validation_code_hash: self.validation_code.hash(),
				para_head: self.para_head_hash.unwrap_or_else(|| self.head_data.hash()),
				erasure_root: Default::default(),
				signature: dummy_collator_signature(),
				collator: dummy_collator(),
			},
			commitments: CandidateCommitments {
				head_data: self.head_data,
				new_validation_code: self.new_validation_code,
				hrmp_watermark: self.hrmp_watermark,
				..Default::default()
			},
		}
	}
}

pub(crate) fn make_vdata_hash(para_id: ParaId) -> Option<Hash> {
	let relay_parent_number = <frame_system::Pallet<Test>>::block_number() - 1;
	make_vdata_hash_with_block_number(para_id, relay_parent_number)
}

fn make_vdata_hash_with_block_number(
	para_id: ParaId,
	relay_parent_number: BlockNumber,
) -> Option<Hash> {
	let persisted_validation_data = crate::util::make_persisted_validation_data::<Test>(
		para_id,
		relay_parent_number,
		Default::default(),
	)?;
	Some(persisted_validation_data.hash())
}

/// Wrapper around `sanitize_bitfields` with less parameters.
fn simple_sanitize_bitfields(
	unchecked_bitfields: UncheckedSignedAvailabilityBitfields,
	disputed_bitfield: DisputedBitfield,
	expected_bits: usize,
) -> SignedAvailabilityBitfields {
	let parent_hash = frame_system::Pallet::<Test>::parent_hash();
	let session_index = shared::Pallet::<Test>::session_index();
	let validators = shared::Pallet::<Test>::active_validator_keys();

	crate::paras_inherent::sanitize_bitfields::<Test>(
		unchecked_bitfields,
		disputed_bitfield,
		expected_bits,
		parent_hash,
		session_index,
		&validators,
	)
}
/// Process a set of already sanitized bitfields.
pub(crate) fn process_bitfields(
	expected_bits: usize,
	signed_bitfields: SignedAvailabilityBitfields,
	core_lookup: impl Fn(CoreIndex) -> Option<ParaId>,
) -> Vec<(CoreIndex, CandidateHash)> {
	let validators = shared::Pallet::<Test>::active_validator_keys();

	ParaInclusion::update_pending_availability_and_get_freed_cores::<_>(
		expected_bits,
		&validators[..],
		signed_bitfields,
		core_lookup,
	)
}

#[test]
fn collect_pending_cleans_up_pending() {
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let thread_a = ParaId::from(3_u32);

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	new_test_ext(genesis_config(paras)).execute_with(|| {
		let default_candidate = TestCandidateBuilder::default().build();
		<PendingAvailability<Test>>::insert(
			chain_a,
			CandidatePendingAvailability {
				core: CoreIndex::from(0),
				hash: default_candidate.hash(),
				descriptor: default_candidate.descriptor.clone(),
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
				backers: default_backing_bitfield(),
				backing_group: GroupIndex::from(0),
			},
		);
		PendingAvailabilityCommitments::<Test>::insert(
			chain_a,
			default_candidate.commitments.clone(),
		);

		<PendingAvailability<Test>>::insert(
			&chain_b,
			CandidatePendingAvailability {
				core: CoreIndex::from(1),
				hash: default_candidate.hash(),
				descriptor: default_candidate.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
				backers: default_backing_bitfield(),
				backing_group: GroupIndex::from(1),
			},
		);
		PendingAvailabilityCommitments::<Test>::insert(chain_b, default_candidate.commitments);

		run_to_block(5, |_| None);

		assert!(<PendingAvailability<Test>>::get(&chain_a).is_some());
		assert!(<PendingAvailability<Test>>::get(&chain_b).is_some());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_a).is_some());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_b).is_some());

		ParaInclusion::collect_pending(|core, _since| core == CoreIndex::from(0));

		assert!(<PendingAvailability<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailability<Test>>::get(&chain_b).is_some());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_b).is_some());
	});
}

#[test]
fn bitfield_checks() {
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let thread_a = ParaId::from(3_u32);

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras.clone())).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		let signing_context =
			SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let core_lookup = |core| match core {
			core if core == CoreIndex::from(0) => Some(chain_a),
			core if core == CoreIndex::from(1) => Some(chain_b),
			core if core == CoreIndex::from(2) => Some(thread_a),
			core if core == CoreIndex::from(3) => None, // for the expected_cores() + 1 test below.
			_ => panic!("out of bounds for testing"),
		};

		// too many bits in bitfield
		{
			let mut bare_bitfield = default_bitfield();
			bare_bitfield.0.push(false);
			let signed = sign_bitfield(
				&keystore,
				&validators[0],
				ValidatorIndex(0),
				bare_bitfield,
				&signing_context,
			);

			let checked_bitfields = simple_sanitize_bitfields(
				vec![signed.into()],
				DisputedBitfield::zeros(expected_bits()),
				expected_bits(),
			);
			assert_eq!(
				checked_bitfields.len(),
				0,
				"Bitfield has wrong size, it should have been filtered."
			);
		}

		// not enough bits
		{
			let bare_bitfield = default_bitfield();
			let signed = sign_bitfield(
				&keystore,
				&validators[0],
				ValidatorIndex(0),
				bare_bitfield,
				&signing_context,
			);

			let checked_bitfields = simple_sanitize_bitfields(
				vec![signed.into()],
				DisputedBitfield::zeros(expected_bits()),
				expected_bits() + 1,
			);
			assert_eq!(
				checked_bitfields.len(),
				0,
				"Bitfield has wrong size, it should have been filtered."
			);
		}

		// non-pending bit set.
		{
			let mut bare_bitfield = default_bitfield();
			*bare_bitfield.0.get_mut(0).unwrap() = true;
			let signed = sign_bitfield(
				&keystore,
				&validators[0],
				ValidatorIndex(0),
				bare_bitfield,
				&signing_context,
			);

			// the threshold to free a core is 4 availability votes, but we only expect 1 valid
			// valid bitfield because `signed_0` will get skipped for being out of order.
			let checked_bitfields = simple_sanitize_bitfields(
				vec![signed.into()],
				DisputedBitfield::zeros(expected_bits()),
				expected_bits(),
			);
			assert_eq!(checked_bitfields.len(), 1, "No bitfields should have been filtered!");

			let x = process_bitfields(expected_bits(), checked_bitfields, core_lookup);
			assert!(x.is_empty(), "No core should be freed.");
		}

		// empty bitfield signed: always ok, but kind of useless.
		{
			let bare_bitfield = default_bitfield();
			let signed = sign_bitfield(
				&keystore,
				&validators[0],
				ValidatorIndex(0),
				bare_bitfield,
				&signing_context,
			);
			let checked_bitfields = simple_sanitize_bitfields(
				vec![signed.into()],
				DisputedBitfield::zeros(expected_bits()),
				expected_bits(),
			);
			assert_eq!(checked_bitfields.len(), 1, "No bitfields should have been filtered!");

			let x = process_bitfields(expected_bits(), checked_bitfields, core_lookup);
			assert!(x.is_empty(), "No core should be freed.");
		}

		// bitfield signed with pending bit signed.
		{
			let mut bare_bitfield = default_bitfield();

			assert_eq!(core_lookup(CoreIndex::from(0)), Some(chain_a));

			let default_candidate = TestCandidateBuilder::default().build();
			<PendingAvailability<Test>>::insert(
				chain_a,
				CandidatePendingAvailability {
					core: CoreIndex::from(0),
					hash: default_candidate.hash(),
					descriptor: default_candidate.descriptor,
					availability_votes: default_availability_votes(),
					relay_parent_number: 0,
					backed_in_number: 0,
					backers: default_backing_bitfield(),
					backing_group: GroupIndex::from(0),
				},
			);
			PendingAvailabilityCommitments::<Test>::insert(chain_a, default_candidate.commitments);

			*bare_bitfield.0.get_mut(0).unwrap() = true;
			let signed = sign_bitfield(
				&keystore,
				&validators[0],
				ValidatorIndex(0),
				bare_bitfield,
				&signing_context,
			);

			let checked_bitfields = simple_sanitize_bitfields(
				vec![signed.into()],
				DisputedBitfield::zeros(expected_bits()),
				expected_bits(),
			);
			assert_eq!(checked_bitfields.len(), 1, "No bitfields should have been filtered!");

			let x = process_bitfields(expected_bits(), checked_bitfields, core_lookup);
			assert!(x.is_empty(), "No core should be freed.");

			<PendingAvailability<Test>>::remove(chain_a);
			PendingAvailabilityCommitments::<Test>::remove(chain_a);
		}

		// bitfield signed with pending bit signed, but no commitments.
		{
			let mut bare_bitfield = default_bitfield();

			assert_eq!(core_lookup(CoreIndex::from(0)), Some(chain_a));

			let default_candidate = TestCandidateBuilder::default().build();
			<PendingAvailability<Test>>::insert(
				chain_a,
				CandidatePendingAvailability {
					core: CoreIndex::from(0),
					hash: default_candidate.hash(),
					descriptor: default_candidate.descriptor,
					availability_votes: default_availability_votes(),
					relay_parent_number: 0,
					backed_in_number: 0,
					backers: default_backing_bitfield(),
					backing_group: GroupIndex::from(0),
				},
			);

			*bare_bitfield.0.get_mut(0).unwrap() = true;
			let signed = sign_bitfield(
				&keystore,
				&validators[0],
				ValidatorIndex(0),
				bare_bitfield,
				&signing_context,
			);

			let checked_bitfields = simple_sanitize_bitfields(
				vec![signed.into()],
				DisputedBitfield::zeros(expected_bits()),
				expected_bits(),
			);
			assert_eq!(checked_bitfields.len(), 1, "No bitfields should have been filtered!");

			let x = process_bitfields(expected_bits(), checked_bitfields, core_lookup);
			// no core is freed
			assert!(x.is_empty(), "No core should be freed.");
		}
	});
}

#[test]
fn availability_threshold_is_supermajority() {
	assert_eq!(3, availability_threshold(4));
	assert_eq!(5, availability_threshold(6));
	assert_eq!(7, availability_threshold(9));
}

#[test]
fn supermajority_bitfields_trigger_availability() {
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let thread_a = ParaId::from(3_u32);

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras)).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		let signing_context =
			SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let core_lookup = |core| match core {
			core if core == CoreIndex::from(0) => Some(chain_a),
			core if core == CoreIndex::from(1) => Some(chain_b),
			core if core == CoreIndex::from(2) => Some(thread_a),
			_ => panic!("Core out of bounds for 2 parachains and 1 parathread core."),
		};

		let candidate_a = TestCandidateBuilder {
			para_id: chain_a,
			head_data: vec![1, 2, 3, 4].into(),
			..Default::default()
		}
		.build();

		<PendingAvailability<Test>>::insert(
			chain_a,
			CandidatePendingAvailability {
				core: CoreIndex::from(0),
				hash: candidate_a.hash(),
				descriptor: candidate_a.clone().descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
				backers: backing_bitfield(&[3, 4]),
				backing_group: GroupIndex::from(0),
			},
		);
		PendingAvailabilityCommitments::<Test>::insert(chain_a, candidate_a.clone().commitments);

		let candidate_b = TestCandidateBuilder {
			para_id: chain_b,
			head_data: vec![5, 6, 7, 8].into(),
			..Default::default()
		}
		.build();

		<PendingAvailability<Test>>::insert(
			chain_b,
			CandidatePendingAvailability {
				core: CoreIndex::from(1),
				hash: candidate_b.hash(),
				descriptor: candidate_b.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
				backers: backing_bitfield(&[0, 2]),
				backing_group: GroupIndex::from(1),
			},
		);
		PendingAvailabilityCommitments::<Test>::insert(chain_b, candidate_b.commitments);

		// this bitfield signals that a and b are available.
		let a_and_b_available = {
			let mut bare_bitfield = default_bitfield();
			*bare_bitfield.0.get_mut(0).unwrap() = true;
			*bare_bitfield.0.get_mut(1).unwrap() = true;

			bare_bitfield
		};

		// this bitfield signals that only a is available.
		let a_available = {
			let mut bare_bitfield = default_bitfield();
			*bare_bitfield.0.get_mut(0).unwrap() = true;

			bare_bitfield
		};

		let threshold = availability_threshold(validators.len());

		// 4 of 5 first value >= 2/3
		assert_eq!(threshold, 4);

		let signed_bitfields = validators
			.iter()
			.enumerate()
			.filter_map(|(i, key)| {
				let to_sign = if i < 3 {
					a_and_b_available.clone()
				} else if i < 4 {
					a_available.clone()
				} else {
					// sign nothing.
					return None
				};

				Some(
					sign_bitfield(
						&keystore,
						key,
						ValidatorIndex(i as _),
						to_sign,
						&signing_context,
					)
					.into(),
				)
			})
			.collect::<Vec<_>>();

		let old_len = signed_bitfields.len();
		let checked_bitfields = simple_sanitize_bitfields(
			signed_bitfields,
			DisputedBitfield::zeros(expected_bits()),
			expected_bits(),
		);
		assert_eq!(checked_bitfields.len(), old_len, "No bitfields should have been filtered!");

		// only chain A's core is freed.
		let v = process_bitfields(expected_bits(), checked_bitfields, core_lookup);
		assert_eq!(vec![(CoreIndex(0), candidate_a.hash())], v);

		// chain A had 4 signing off, which is >= threshold.
		// chain B has 3 signing off, which is < threshold.
		assert!(<PendingAvailability<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_b).is_some());
		assert_eq!(<PendingAvailability<Test>>::get(&chain_b).unwrap().availability_votes, {
			// check that votes from first 3 were tracked.

			let mut votes = default_availability_votes();
			*votes.get_mut(0).unwrap() = true;
			*votes.get_mut(1).unwrap() = true;
			*votes.get_mut(2).unwrap() = true;

			votes
		});

		// and check that chain head was enacted.
		assert_eq!(Paras::para_head(&chain_a), Some(vec![1, 2, 3, 4].into()));

		// Check that rewards are applied.
		{
			let rewards = crate::mock::availability_rewards();

			assert_eq!(rewards.len(), 4);
			assert_eq!(rewards.get(&ValidatorIndex(0)).unwrap(), &1);
			assert_eq!(rewards.get(&ValidatorIndex(1)).unwrap(), &1);
			assert_eq!(rewards.get(&ValidatorIndex(2)).unwrap(), &1);
			assert_eq!(rewards.get(&ValidatorIndex(3)).unwrap(), &1);
		}

		{
			let rewards = crate::mock::backing_rewards();

			assert_eq!(rewards.len(), 2);
			assert_eq!(rewards.get(&ValidatorIndex(3)).unwrap(), &1);
			assert_eq!(rewards.get(&ValidatorIndex(4)).unwrap(), &1);
		}
	});
}

#[test]
fn candidate_checks() {
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let thread_a = ParaId::from(3_u32);

	// The block number of the relay-parent for testing.
	const RELAY_PARENT_NUM: BlockNumber = 4;

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras)).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		run_to_block(5, |_| None);

		let signing_context =
			SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let group_validators = |group_index: GroupIndex| {
			match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1]),
				group_index if group_index == GroupIndex::from(1) => Some(vec![2, 3]),
				group_index if group_index == GroupIndex::from(2) => Some(vec![4]),
				_ => panic!("Group index out of bounds for 2 parachains and 1 parathread core"),
			}
			.map(|m| m.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
		};

		// When processing candidates, we compute the group index from scheduler.
		let validator_groups = vec![
			vec![ValidatorIndex(0), ValidatorIndex(1)],
			vec![ValidatorIndex(2), ValidatorIndex(3)],
			vec![ValidatorIndex(4)],
		];
		Scheduler::set_validator_groups(validator_groups);

		let entry_ttl = 10_000;
		let thread_collator: CollatorId = Sr25519Keyring::Two.public().into();
		let chain_a_assignment = CoreAssignment {
			core: CoreIndex::from(0),
			paras_entry: ParasEntry::new(Assignment::new(chain_a), entry_ttl),
		};

		let chain_b_assignment = CoreAssignment {
			core: CoreIndex::from(1),
			paras_entry: ParasEntry::new(Assignment::new(chain_b), entry_ttl),
		};

		let thread_a_assignment = CoreAssignment {
			core: CoreIndex::from(2),
			paras_entry: ParasEntry::new(Assignment::new(thread_a), entry_ttl),
		};

		let allowed_relay_parents = default_allowed_relay_parent_tracker();

		// unscheduled candidate.
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();
			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_b_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::UnscheduledCandidate
			);
		}

		// candidates out of order.
		{
			let mut candidate_a = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();
			let mut candidate_b = TestCandidateBuilder {
				para_id: chain_b,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(2),
				persisted_validation_data_hash: make_vdata_hash(chain_b).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate_a);

			collator_sign_candidate(Sr25519Keyring::Two, &mut candidate_b);

			let backed_a = back_candidate(
				candidate_a,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			let backed_b = back_candidate(
				candidate_b,
				&validators,
				group_validators(GroupIndex::from(1)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			// out-of-order manifests as unscheduled.
			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed_b, backed_a],
					vec![chain_a_assignment.clone(), chain_b_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::UnscheduledCandidate
			);
		}

		// candidate not backed.
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();
			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Lacking,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::InsufficientBacking
			);
		}

		// one of candidates is not based on allowed relay parent.
		{
			let wrong_parent_hash = Hash::repeat_byte(222);
			assert!(System::parent_hash() != wrong_parent_hash);

			let mut candidate_a = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: wrong_parent_hash,
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				..Default::default()
			}
			.build();

			let mut candidate_b = TestCandidateBuilder {
				para_id: chain_b,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(2),
				persisted_validation_data_hash: make_vdata_hash(chain_b).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate_a);

			collator_sign_candidate(Sr25519Keyring::Two, &mut candidate_b);

			let backed_a = back_candidate(
				candidate_a,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			let backed_b = back_candidate(
				candidate_b,
				&validators,
				group_validators(GroupIndex::from(1)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed_b, backed_a],
					vec![chain_a_assignment.clone(), chain_b_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::DisallowedRelayParent
			);
		}

		// candidate not well-signed by collator.
		{
			let mut candidate = TestCandidateBuilder {
				para_id: thread_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(thread_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			assert_eq!(CollatorId::from(Sr25519Keyring::Two.public()), thread_collator);
			collator_sign_candidate(Sr25519Keyring::Two, &mut candidate);

			// change the candidate after signing.
			candidate.descriptor.pov_hash = Hash::repeat_byte(2);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(2)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![thread_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::NotCollatorSigned
			);
		}

		// para occupied - reject.
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			let candidate = TestCandidateBuilder::default().build();
			<PendingAvailability<Test>>::insert(
				&chain_a,
				CandidatePendingAvailability {
					core: CoreIndex::from(0),
					hash: candidate.hash(),
					descriptor: candidate.descriptor,
					availability_votes: default_availability_votes(),
					relay_parent_number: 3,
					backed_in_number: 4,
					backers: default_backing_bitfield(),
					backing_group: GroupIndex::from(0),
				},
			);
			<PendingAvailabilityCommitments<Test>>::insert(&chain_a, candidate.commitments);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::CandidateScheduledBeforeParaFree
			);

			<PendingAvailability<Test>>::remove(&chain_a);
			<PendingAvailabilityCommitments<Test>>::remove(&chain_a);
		}

		// messed up commitments storage - do not panic - reject.
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			// this is not supposed to happen
			<PendingAvailabilityCommitments<Test>>::insert(&chain_a, candidate.commitments.clone());

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::CandidateScheduledBeforeParaFree
			);

			<PendingAvailabilityCommitments<Test>>::remove(&chain_a);
		}

		// interfering code upgrade - reject
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				new_validation_code: Some(vec![5, 6, 7, 8].into()),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			{
				let cfg = Configuration::config();
				let expected_at = 10 + cfg.validation_upgrade_delay;
				assert_eq!(expected_at, 12);
				Paras::schedule_code_upgrade(chain_a, vec![1, 2, 3, 4].into(), expected_at, &cfg);
			}

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::PrematureCodeUpgrade
			);
		}

		// Bad validation data hash - reject
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: [42u8; 32].into(),
				hrmp_watermark: RELAY_PARENT_NUM,
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_eq!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Err(Error::<Test>::ValidationDataHashMismatch.into()),
			);
		}

		// bad validation code hash
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				validation_code: ValidationCode(vec![1]),
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::InvalidValidationCodeHash
			);
		}

		// Para head hash in descriptor doesn't match head data
		{
			let mut candidate = TestCandidateBuilder {
				para_id: chain_a,
				relay_parent: System::parent_hash(),
				pov_hash: Hash::repeat_byte(1),
				persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
				hrmp_watermark: RELAY_PARENT_NUM,
				para_head_hash: Some(Hash::random()),
				..Default::default()
			}
			.build();

			collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

			let backed = back_candidate(
				candidate,
				&validators,
				group_validators(GroupIndex::from(0)).unwrap().as_ref(),
				&keystore,
				&signing_context,
				BackingKind::Threshold,
			);

			assert_noop!(
				ParaInclusion::process_candidates(
					&allowed_relay_parents,
					vec![backed],
					vec![chain_a_assignment.clone()],
					&group_validators,
				),
				Error::<Test>::ParaHeadMismatch
			);
		}
	});
}

#[test]
fn backing_works() {
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let thread_a = ParaId::from(3_u32);

	// The block number of the relay-parent for testing.
	const RELAY_PARENT_NUM: BlockNumber = 4;

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras)).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		run_to_block(5, |_| None);

		let signing_context =
			SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let group_validators = |group_index: GroupIndex| {
			match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1]),
				group_index if group_index == GroupIndex::from(1) => Some(vec![2, 3]),
				group_index if group_index == GroupIndex::from(2) => Some(vec![4]),
				_ => panic!("Group index out of bounds for 2 parachains and 1 parathread core"),
			}
			.map(|vs| vs.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
		};

		// When processing candidates, we compute the group index from scheduler.
		let validator_groups = vec![
			vec![ValidatorIndex(0), ValidatorIndex(1)],
			vec![ValidatorIndex(2), ValidatorIndex(3)],
			vec![ValidatorIndex(4)],
		];
		Scheduler::set_validator_groups(validator_groups);

		let allowed_relay_parents = default_allowed_relay_parent_tracker();

		let entry_ttl = 10_000;
		let chain_a_assignment = CoreAssignment {
			core: CoreIndex::from(0),
			paras_entry: ParasEntry::new(Assignment::new(chain_a), entry_ttl),
		};

		let chain_b_assignment = CoreAssignment {
			core: CoreIndex::from(1),
			paras_entry: ParasEntry::new(Assignment::new(chain_b), entry_ttl),
		};

		let thread_a_assignment = CoreAssignment {
			core: CoreIndex::from(2),
			paras_entry: ParasEntry::new(Assignment::new(thread_a), entry_ttl),
		};

		let mut candidate_a = TestCandidateBuilder {
			para_id: chain_a,
			relay_parent: System::parent_hash(),
			pov_hash: Hash::repeat_byte(1),
			persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
			hrmp_watermark: RELAY_PARENT_NUM,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::One, &mut candidate_a);

		let mut candidate_b = TestCandidateBuilder {
			para_id: chain_b,
			relay_parent: System::parent_hash(),
			pov_hash: Hash::repeat_byte(2),
			persisted_validation_data_hash: make_vdata_hash(chain_b).unwrap(),
			hrmp_watermark: RELAY_PARENT_NUM,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::One, &mut candidate_b);

		let mut candidate_c = TestCandidateBuilder {
			para_id: thread_a,
			relay_parent: System::parent_hash(),
			pov_hash: Hash::repeat_byte(3),
			persisted_validation_data_hash: make_vdata_hash(thread_a).unwrap(),
			hrmp_watermark: RELAY_PARENT_NUM,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::Two, &mut candidate_c);

		let backed_a = back_candidate(
			candidate_a.clone(),
			&validators,
			group_validators(GroupIndex::from(0)).unwrap().as_ref(),
			&keystore,
			&signing_context,
			BackingKind::Threshold,
		);

		let backed_b = back_candidate(
			candidate_b.clone(),
			&validators,
			group_validators(GroupIndex::from(1)).unwrap().as_ref(),
			&keystore,
			&signing_context,
			BackingKind::Threshold,
		);

		let backed_c = back_candidate(
			candidate_c.clone(),
			&validators,
			group_validators(GroupIndex::from(2)).unwrap().as_ref(),
			&keystore,
			&signing_context,
			BackingKind::Threshold,
		);

		let backed_candidates = vec![backed_a.clone(), backed_b.clone(), backed_c];
		let get_backing_group_idx = {
			// the order defines the group implicitly for this test case
			let backed_candidates_with_groups = backed_candidates
				.iter()
				.enumerate()
				.map(|(idx, backed_candidate)| (backed_candidate.hash(), GroupIndex(idx as _)))
				.collect::<Vec<_>>();

			move |candidate_hash_x: CandidateHash| -> Option<GroupIndex> {
				backed_candidates_with_groups.iter().find_map(|(candidate_hash, grp)| {
					if *candidate_hash == candidate_hash_x {
						Some(*grp)
					} else {
						None
					}
				})
			}
		};

		let ProcessedCandidates {
			core_indices: occupied_cores,
			candidate_receipt_with_backing_validator_indices,
		} = ParaInclusion::process_candidates(
			&allowed_relay_parents,
			backed_candidates.clone(),
			vec![
				chain_a_assignment.clone(),
				chain_b_assignment.clone(),
				thread_a_assignment.clone(),
			],
			&group_validators,
		)
		.expect("candidates scheduled, in order, and backed");

		assert_eq!(
			occupied_cores,
			vec![
				(CoreIndex::from(0), chain_a),
				(CoreIndex::from(1), chain_b),
				(CoreIndex::from(2), thread_a)
			]
		);

		// Transform the votes into the setup we expect
		let expected = {
			let mut intermediate = std::collections::HashMap::<
				CandidateHash,
				(CandidateReceipt, Vec<(ValidatorIndex, ValidityAttestation)>),
			>::new();
			backed_candidates.into_iter().for_each(|backed_candidate| {
				let candidate_receipt_with_backers = intermediate
					.entry(backed_candidate.hash())
					.or_insert_with(|| (backed_candidate.receipt(), Vec::new()));

				assert_eq!(
					backed_candidate.validity_votes.len(),
					backed_candidate.validator_indices.count_ones()
				);
				candidate_receipt_with_backers.1.extend(
					backed_candidate
						.validator_indices
						.iter()
						.enumerate()
						.filter(|(_, signed)| **signed)
						.zip(backed_candidate.validity_votes.iter().cloned())
						.filter_map(|((validator_index_within_group, _), attestation)| {
							let grp_idx = get_backing_group_idx(backed_candidate.hash()).unwrap();
							group_validators(grp_idx).map(|validator_indices| {
								(validator_indices[validator_index_within_group], attestation)
							})
						}),
				);
			});
			intermediate.into_values().collect::<Vec<_>>()
		};

		// sort, since we use a hashmap above
		let assure_candidate_sorting = |mut candidate_receipts_with_backers: Vec<(
			CandidateReceipt,
			Vec<(ValidatorIndex, ValidityAttestation)>,
		)>| {
			candidate_receipts_with_backers.sort_by(|(cr1, _), (cr2, _)| {
				cr1.descriptor().para_id.cmp(&cr2.descriptor().para_id)
			});
			candidate_receipts_with_backers
		};
		assert_eq!(
			assure_candidate_sorting(expected),
			assure_candidate_sorting(candidate_receipt_with_backing_validator_indices)
		);

		let backers = {
			let num_backers = minimum_backing_votes(group_validators(GroupIndex(0)).unwrap().len());
			backing_bitfield(&(0..num_backers).collect::<Vec<_>>())
		};
		assert_eq!(
			<PendingAvailability<Test>>::get(&chain_a),
			Some(CandidatePendingAvailability {
				core: CoreIndex::from(0),
				hash: candidate_a.hash(),
				descriptor: candidate_a.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: System::block_number() - 1,
				backed_in_number: System::block_number(),
				backers,
				backing_group: GroupIndex::from(0),
			})
		);
		assert_eq!(
			<PendingAvailabilityCommitments<Test>>::get(&chain_a),
			Some(candidate_a.commitments),
		);

		let backers = {
			let num_backers = minimum_backing_votes(group_validators(GroupIndex(0)).unwrap().len());
			backing_bitfield(&(0..num_backers).map(|v| v + 2).collect::<Vec<_>>())
		};
		assert_eq!(
			<PendingAvailability<Test>>::get(&chain_b),
			Some(CandidatePendingAvailability {
				core: CoreIndex::from(1),
				hash: candidate_b.hash(),
				descriptor: candidate_b.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: System::block_number() - 1,
				backed_in_number: System::block_number(),
				backers,
				backing_group: GroupIndex::from(1),
			})
		);
		assert_eq!(
			<PendingAvailabilityCommitments<Test>>::get(&chain_b),
			Some(candidate_b.commitments),
		);

		assert_eq!(
			<PendingAvailability<Test>>::get(&thread_a),
			Some(CandidatePendingAvailability {
				core: CoreIndex::from(2),
				hash: candidate_c.hash(),
				descriptor: candidate_c.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: System::block_number() - 1,
				backed_in_number: System::block_number(),
				backers: backing_bitfield(&[4]),
				backing_group: GroupIndex::from(2),
			})
		);
		assert_eq!(
			<PendingAvailabilityCommitments<Test>>::get(&thread_a),
			Some(candidate_c.commitments),
		);
	});
}

#[test]
fn can_include_candidate_with_ok_code_upgrade() {
	let chain_a = ParaId::from(1_u32);

	// The block number of the relay-parent for testing.
	const RELAY_PARENT_NUM: BlockNumber = 4;

	let paras = vec![(chain_a, ParaKind::Parachain)];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras)).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		run_to_block(5, |_| None);

		let signing_context =
			SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let group_validators = |group_index: GroupIndex| {
			match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1, 2, 3, 4]),
				_ => panic!("Group index out of bounds for 1 parachain"),
			}
			.map(|vs| vs.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
		};

		// When processing candidates, we compute the group index from scheduler.
		let validator_groups = vec![vec![
			ValidatorIndex(0),
			ValidatorIndex(1),
			ValidatorIndex(2),
			ValidatorIndex(3),
			ValidatorIndex(4),
		]];
		Scheduler::set_validator_groups(validator_groups);

		let allowed_relay_parents = default_allowed_relay_parent_tracker();
		let entry_ttl = 10_000;
		let chain_a_assignment = CoreAssignment {
			core: CoreIndex::from(0),
			paras_entry: ParasEntry::new(Assignment::new(chain_a), entry_ttl),
		};

		let mut candidate_a = TestCandidateBuilder {
			para_id: chain_a,
			relay_parent: System::parent_hash(),
			pov_hash: Hash::repeat_byte(1),
			persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
			new_validation_code: Some(vec![1, 2, 3].into()),
			hrmp_watermark: RELAY_PARENT_NUM,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::One, &mut candidate_a);

		let backed_a = back_candidate(
			candidate_a.clone(),
			&validators,
			group_validators(GroupIndex::from(0)).unwrap().as_ref(),
			&keystore,
			&signing_context,
			BackingKind::Threshold,
		);

		let ProcessedCandidates { core_indices: occupied_cores, .. } =
			ParaInclusion::process_candidates(
				&allowed_relay_parents,
				vec![backed_a],
				vec![chain_a_assignment.clone()],
				&group_validators,
			)
			.expect("candidates scheduled, in order, and backed");

		assert_eq!(occupied_cores, vec![(CoreIndex::from(0), chain_a)]);

		let backers = {
			let num_backers = minimum_backing_votes(group_validators(GroupIndex(0)).unwrap().len());
			backing_bitfield(&(0..num_backers).collect::<Vec<_>>())
		};
		assert_eq!(
			<PendingAvailability<Test>>::get(&chain_a),
			Some(CandidatePendingAvailability {
				core: CoreIndex::from(0),
				hash: candidate_a.hash(),
				descriptor: candidate_a.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: System::block_number() - 1,
				backed_in_number: System::block_number(),
				backers,
				backing_group: GroupIndex::from(0),
			})
		);
		assert_eq!(
			<PendingAvailabilityCommitments<Test>>::get(&chain_a),
			Some(candidate_a.commitments),
		);
	});
}

#[test]
fn check_allowed_relay_parents() {
	let chain_a = ParaId::from(1);
	let chain_b = ParaId::from(2);
	let thread_a = ParaId::from(3);

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);
	let mut config = genesis_config(paras);
	config.configuration.config.group_rotation_frequency = 1;

	new_test_ext(config).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		run_to_block(5, |_| None);

		let group_validators = |group_index: GroupIndex| {
			match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1]),
				group_index if group_index == GroupIndex::from(1) => Some(vec![2, 3]),
				group_index if group_index == GroupIndex::from(2) => Some(vec![4]),
				_ => panic!("Group index out of bounds for 2 parachains and 1 parathread core"),
			}
			.map(|vs| vs.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
		};

		// When processing candidates, we compute the group index from scheduler.
		let validator_groups = vec![
			vec![ValidatorIndex(0), ValidatorIndex(1)],
			vec![ValidatorIndex(2), ValidatorIndex(3)],
			vec![ValidatorIndex(4)],
		];
		Scheduler::set_validator_groups(validator_groups);

		// Base each candidate on one of allowed relay parents.
		//
		// Note that the group rotation frequency is set to 1 above,
		// which means groups shift at each relay parent.
		//
		// For example, candidate `a` is based on block 1,
		// thus it will be included in block 2, its group index is
		// core = 0 shifted 2 times: one for group rotation and one for
		// fetching the group assigned to the next block.
		//
		// Candidates `b` and `c` are constructed accordingly.

		let relay_parent_a = (1, Hash::repeat_byte(0x1));
		let relay_parent_b = (2, Hash::repeat_byte(0x2));
		let relay_parent_c = (3, Hash::repeat_byte(0x3));

		let mut allowed_relay_parents = AllowedRelayParentsTracker::default();
		let max_ancestry_len = 3;
		allowed_relay_parents.update(
			relay_parent_a.1,
			Hash::zero(),
			relay_parent_a.0,
			max_ancestry_len,
		);
		allowed_relay_parents.update(
			relay_parent_b.1,
			Hash::zero(),
			relay_parent_b.0,
			max_ancestry_len,
		);
		allowed_relay_parents.update(
			relay_parent_c.1,
			Hash::zero(),
			relay_parent_c.0,
			max_ancestry_len,
		);

		let chain_a_assignment = CoreAssignment {
			core: CoreIndex::from(0),
			paras_entry: ParasEntry {
				assignment: Assignment { para_id: chain_a },
				availability_timeouts: 0,
				ttl: 5,
			},
		};

		let chain_b_assignment = CoreAssignment {
			core: CoreIndex::from(1),
			paras_entry: ParasEntry {
				assignment: Assignment { para_id: chain_b },
				availability_timeouts: 0,
				ttl: 5,
			},
		};

		let thread_a_assignment = CoreAssignment {
			core: CoreIndex::from(2),
			paras_entry: ParasEntry::new(Assignment::new(thread_a), 5),
		};

		let mut candidate_a = TestCandidateBuilder {
			para_id: chain_a,
			relay_parent: relay_parent_a.1,
			pov_hash: Hash::repeat_byte(1),
			persisted_validation_data_hash: make_vdata_hash_with_block_number(
				chain_a,
				relay_parent_a.0,
			)
			.unwrap(),
			hrmp_watermark: relay_parent_a.0,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::One, &mut candidate_a);
		let signing_context_a = SigningContext { parent_hash: relay_parent_a.1, session_index: 5 };

		let mut candidate_b = TestCandidateBuilder {
			para_id: chain_b,
			relay_parent: relay_parent_b.1,
			pov_hash: Hash::repeat_byte(2),
			persisted_validation_data_hash: make_vdata_hash_with_block_number(
				chain_b,
				relay_parent_b.0,
			)
			.unwrap(),
			hrmp_watermark: relay_parent_b.0,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::One, &mut candidate_b);
		let signing_context_b = SigningContext { parent_hash: relay_parent_b.1, session_index: 5 };

		let mut candidate_c = TestCandidateBuilder {
			para_id: thread_a,
			relay_parent: relay_parent_c.1,
			pov_hash: Hash::repeat_byte(3),
			persisted_validation_data_hash: make_vdata_hash_with_block_number(
				thread_a,
				relay_parent_c.0,
			)
			.unwrap(),
			hrmp_watermark: relay_parent_c.0,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::Two, &mut candidate_c);
		let signing_context_c = SigningContext { parent_hash: relay_parent_c.1, session_index: 5 };

		let backed_a = back_candidate(
			candidate_a.clone(),
			&validators,
			group_validators(GroupIndex::from(2)).unwrap().as_ref(),
			&keystore,
			&signing_context_a,
			BackingKind::Threshold,
		);

		let backed_b = back_candidate(
			candidate_b.clone(),
			&validators,
			group_validators(GroupIndex::from(1)).unwrap().as_ref(),
			&keystore,
			&signing_context_b,
			BackingKind::Threshold,
		);

		let backed_c = back_candidate(
			candidate_c.clone(),
			&validators,
			group_validators(GroupIndex::from(0)).unwrap().as_ref(),
			&keystore,
			&signing_context_c,
			BackingKind::Threshold,
		);

		let backed_candidates = vec![backed_a, backed_b, backed_c];

		ParaInclusion::process_candidates(
			&allowed_relay_parents,
			backed_candidates.clone(),
			vec![
				chain_a_assignment.clone(),
				chain_b_assignment.clone(),
				thread_a_assignment.clone(),
			],
			&group_validators,
		)
		.expect("candidates scheduled, in order, and backed");
	});
}

#[test]
fn session_change_wipes() {
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let thread_a = ParaId::from(3_u32);

	let paras = vec![
		(chain_a, ParaKind::Parachain),
		(chain_b, ParaKind::Parachain),
		(thread_a, ParaKind::Parathread),
	];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras)).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		let validators_new =
			vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie];

		let validator_public_new = validator_pubkeys(&validators_new);

		run_to_block(10, |_| None);

		<AvailabilityBitfields<Test>>::insert(
			&ValidatorIndex(0),
			AvailabilityBitfieldRecord { bitfield: default_bitfield(), submitted_at: 9 },
		);

		<AvailabilityBitfields<Test>>::insert(
			&ValidatorIndex(1),
			AvailabilityBitfieldRecord { bitfield: default_bitfield(), submitted_at: 9 },
		);

		<AvailabilityBitfields<Test>>::insert(
			&ValidatorIndex(4),
			AvailabilityBitfieldRecord { bitfield: default_bitfield(), submitted_at: 9 },
		);

		let candidate = TestCandidateBuilder::default().build();
		<PendingAvailability<Test>>::insert(
			&chain_a,
			CandidatePendingAvailability {
				core: CoreIndex::from(0),
				hash: candidate.hash(),
				descriptor: candidate.descriptor.clone(),
				availability_votes: default_availability_votes(),
				relay_parent_number: 5,
				backed_in_number: 6,
				backers: default_backing_bitfield(),
				backing_group: GroupIndex::from(0),
			},
		);
		<PendingAvailabilityCommitments<Test>>::insert(&chain_a, candidate.commitments.clone());

		<PendingAvailability<Test>>::insert(
			&chain_b,
			CandidatePendingAvailability {
				core: CoreIndex::from(1),
				hash: candidate.hash(),
				descriptor: candidate.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: 6,
				backed_in_number: 7,
				backers: default_backing_bitfield(),
				backing_group: GroupIndex::from(1),
			},
		);
		<PendingAvailabilityCommitments<Test>>::insert(&chain_b, candidate.commitments);

		run_to_block(11, |_| None);

		assert_eq!(shared::Pallet::<Test>::session_index(), 5);

		assert!(<AvailabilityBitfields<Test>>::get(&ValidatorIndex(0)).is_some());
		assert!(<AvailabilityBitfields<Test>>::get(&ValidatorIndex(1)).is_some());
		assert!(<AvailabilityBitfields<Test>>::get(&ValidatorIndex(4)).is_some());

		assert!(<PendingAvailability<Test>>::get(&chain_a).is_some());
		assert!(<PendingAvailability<Test>>::get(&chain_b).is_some());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_a).is_some());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_b).is_some());

		run_to_block(12, |n| match n {
			12 => Some(SessionChangeNotification {
				validators: validator_public_new.clone(),
				queued: Vec::new(),
				prev_config: default_config(),
				new_config: default_config(),
				random_seed: Default::default(),
				session_index: 6,
			}),
			_ => None,
		});

		assert_eq!(shared::Pallet::<Test>::session_index(), 6);

		assert!(<AvailabilityBitfields<Test>>::get(&ValidatorIndex(0)).is_none());
		assert!(<AvailabilityBitfields<Test>>::get(&ValidatorIndex(1)).is_none());
		assert!(<AvailabilityBitfields<Test>>::get(&ValidatorIndex(4)).is_none());

		assert!(<PendingAvailability<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailability<Test>>::get(&chain_b).is_none());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_b).is_none());

		assert!(<AvailabilityBitfields<Test>>::iter().collect::<Vec<_>>().is_empty());
		assert!(<PendingAvailability<Test>>::iter().collect::<Vec<_>>().is_empty());
		assert!(<PendingAvailabilityCommitments<Test>>::iter().collect::<Vec<_>>().is_empty());
	});
}

/// Assert that the encoding of a known `AggregateMessageOrigin` did not change.
#[test]
fn aggregate_origin_decode_regression_check() {
	let ump = AggregateMessageOrigin::Ump(UmpQueueId::Para(u32::MAX.into()));
	let raw = (0u8, 0u8, u32::MAX).encode();
	let decoded = AggregateMessageOrigin::decode_all(&mut &raw[..]);
	assert_eq!(decoded, Ok(ump), "Migration needed for AggregateMessageOrigin");
}

#[test]
fn para_upgrade_delay_scheduled_from_inclusion() {
	let chain_a = ParaId::from(1_u32);

	// The block number of the relay-parent for testing.
	const RELAY_PARENT_NUM: BlockNumber = 4;

	let paras = vec![(chain_a, ParaKind::Parachain)];
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let keystore: KeystorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in validators.iter() {
		Keystore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_public = validator_pubkeys(&validators);

	new_test_ext(genesis_config(paras)).execute_with(|| {
		shared::Pallet::<Test>::set_active_validators_ascending(validator_public.clone());
		shared::Pallet::<Test>::set_session_index(5);

		let new_validation_code: ValidationCode = vec![1, 2, 3, 4, 5].into();
		let new_validation_code_hash = new_validation_code.hash();

		// Otherwise upgrade is no-op.
		assert_ne!(new_validation_code, dummy_validation_code());

		run_to_block(5, |_| None);

		let signing_context =
			SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let group_validators = |group_index: GroupIndex| {
			match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1, 2, 3, 4]),
				_ => panic!("Group index out of bounds for 1 parachain"),
			}
			.map(|vs| vs.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
		};

		// When processing candidates, we compute the group index from scheduler.
		let validator_groups = vec![vec![
			ValidatorIndex(0),
			ValidatorIndex(1),
			ValidatorIndex(2),
			ValidatorIndex(3),
			ValidatorIndex(4),
		]];
		Scheduler::set_validator_groups(validator_groups);

		let core_lookup = |core| match core {
			core if core == CoreIndex::from(0) => Some(chain_a),
			_ => None,
		};

		let allowed_relay_parents = default_allowed_relay_parent_tracker();

		let chain_a_assignment = CoreAssignment {
			core: CoreIndex::from(0),
			paras_entry: ParasEntry {
				assignment: Assignment { para_id: chain_a },
				availability_timeouts: 0,
				ttl: 5,
			},
		};

		let mut candidate_a = TestCandidateBuilder {
			para_id: chain_a,
			relay_parent: System::parent_hash(),
			pov_hash: Hash::repeat_byte(1),
			persisted_validation_data_hash: make_vdata_hash(chain_a).unwrap(),
			new_validation_code: Some(new_validation_code.clone()),
			hrmp_watermark: RELAY_PARENT_NUM,
			..Default::default()
		}
		.build();
		collator_sign_candidate(Sr25519Keyring::One, &mut candidate_a);

		let backed_a = back_candidate(
			candidate_a.clone(),
			&validators,
			group_validators(GroupIndex::from(0)).unwrap().as_ref(),
			&keystore,
			&signing_context,
			BackingKind::Threshold,
		);

		let ProcessedCandidates { core_indices: occupied_cores, .. } =
			ParaInclusion::process_candidates(
				&allowed_relay_parents,
				vec![backed_a],
				vec![chain_a_assignment.clone()],
				&group_validators,
			)
			.expect("candidates scheduled, in order, and backed");

		assert_eq!(occupied_cores, vec![(CoreIndex::from(0), chain_a)]);

		// Run a couple of blocks before the inclusion.
		run_to_block(7, |_| None);

		let mut bare_bitfield = default_bitfield();
		*bare_bitfield.0.get_mut(0).unwrap() = true;

		let signed_bitfields = validators
			.iter()
			.enumerate()
			.map(|(i, key)| {
				sign_bitfield(
					&keystore,
					key,
					ValidatorIndex(i as _),
					bare_bitfield.clone(),
					&signing_context,
				)
				.into()
			})
			.collect::<Vec<_>>();

		let checked_bitfields = simple_sanitize_bitfields(
			signed_bitfields,
			DisputedBitfield::zeros(expected_bits()),
			expected_bits(),
		);

		let v = process_bitfields(expected_bits(), checked_bitfields, core_lookup);
		assert_eq!(vec![(CoreIndex(0), candidate_a.hash())], v);

		assert!(<PendingAvailability<Test>>::get(&chain_a).is_none());
		assert!(<PendingAvailabilityCommitments<Test>>::get(&chain_a).is_none());

		let active_vote_state = paras::Pallet::<Test>::active_vote_state(&new_validation_code_hash)
			.expect("prechecking must be initiated");

		let cause = &active_vote_state.causes()[0];
		// Upgrade block is the block of inclusion, not candidate's parent.
		assert_matches!(cause,
			paras::PvfCheckCause::Upgrade { id, included_at }
				if id == &chain_a && included_at == &7
		);
	});
}
