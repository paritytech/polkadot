// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The inclusion module is responsible for inclusion and availability of scheduled parachains
//! and parathreads.
//!
//! It is responsible for carrying candidates from being backable to being backed, and then from backed
//! to included.

use sp_std::prelude::*;
use primitives::{
	parachain::{
		ValidatorId, AbridgedCandidateReceipt, ValidatorIndex, Id as ParaId,
		AvailabilityBitfield as AvailabilityBitfield, SignedAvailabilityBitfields, SigningContext,
		BackedCandidate,
	},
};
use frame_support::{
	decl_storage, decl_module, decl_error, ensure, dispatch::DispatchResult, IterableStorageMap,
	weights::Weight,
	traits::Get,
};
use codec::{Encode, Decode};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use sp_staking::SessionIndex;
use sp_runtime::{DispatchError, traits::{One, Saturating}};

use crate::{configuration, paras, scheduler::{CoreIndex, GroupIndex, CoreAssignment}};

/// A bitfield signed by a validator indicating that it is keeping its piece of the erasure-coding
/// for any backed candidates referred to by a `1` bit available.
#[derive(Encode, Decode)]
#[cfg_attr(test, derive(Debug))]
pub struct AvailabilityBitfieldRecord<N> {
	bitfield: AvailabilityBitfield, // one bit per core.
	submitted_at: N, // for accounting, as meaning of bits may change over time.
}

/// A backed candidate pending availability.
#[derive(Encode, Decode)]
#[cfg_attr(test, derive(Debug))]
pub struct CandidatePendingAvailability<H, N> {
	/// The availability core this is assigned to.
	core: CoreIndex,
	/// The candidate receipt itself.
	receipt: AbridgedCandidateReceipt<H>,
	/// The received availability votes. One bit per validator.
	availability_votes: BitVec<BitOrderLsb0, u8>,
	/// The block number of the relay-parent of the receipt.
	relay_parent_number: N,
	/// The block number of the relay-chain block this was backed in.
	backed_in_number: N,
}

pub trait Trait: system::Trait + paras::Trait + configuration::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as ParaInclusion {
		/// The latest bitfield for each validator, referred to by their index in the validator set.
		AvailabilityBitfields: map hasher(twox_64_concat) ValidatorIndex
			=> Option<AvailabilityBitfieldRecord<T::BlockNumber>>;

		/// Candidates pending availability by `ParaId`.
		PendingAvailability: map hasher(twox_64_concat) ParaId
			=> Option<CandidatePendingAvailability<T::Hash, T::BlockNumber>>;

		/// The current validators, by their parachain session keys.
		Validators get(fn validators) config(validators): Vec<ValidatorId>;

		/// The current session index.
		CurrentSessionIndex: SessionIndex;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Availability bitfield has unexpected size.
		WrongBitfieldSize,
		/// Multiple bitfields submitted by same validator or validators out of order by index.
		BitfieldDuplicateOrUnordered,
		/// Validator index out of bounds.
		ValidatorIndexOutOfBounds,
		/// Invalid signature
		InvalidBitfieldSignature,
		/// Candidate submitted but para not scheduled.
		UnscheduledCandidate,
		/// Candidate scheduled despite pending candidate already existing for the para.
		CandidateScheduledBeforeParaFree,
		/// Candidate included with the wrong collator.
		WrongCollator,
		/// Scheduled cores out of order.
		ScheduledOutOfOrder,
		/// Code upgrade prematurely.
		PrematureCodeUpgrade,
		/// Candidate not in parent context.
		CandidateNotInParentContext,
		/// The bitfield contains a bit relating to an unassigned availability core.
		UnoccupiedBitInBitfield,
		/// Invalid group index in core assignment.
		InvalidGroupIndex,
		/// Insufficient (non-majority) backing.
		InsufficientBacking,
		/// Invalid (bad signature, unknown validator, etc.) backing.
		InvalidBacking,
		/// Collator did not sign PoV.
		NotCollatorSigned,
	}
}

decl_module! {
	/// The parachain-candidate inclusion module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {

	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight { 0 }

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() { }

	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>
	) {
		// unlike most drain methods, drained elements are not cleared on `Drop` of the iterator
		// and require consumption.
		for _ in <PendingAvailability<T>>::drain() { }
		for _ in <AvailabilityBitfields<T>>::drain() { }

		Validators::set(notification.validators.clone()); // substrate forces us to clone, stupidly.
		CurrentSessionIndex::set(notification.session_index);
	}

	/// Process a set of incoming bitfields. Return a vec of cores freed by candidates
	/// becoming available.
	pub(crate) fn process_bitfields(
		signed_bitfields: SignedAvailabilityBitfields,
		core_lookup: impl Fn(CoreIndex) -> Option<ParaId>,
	) -> Result<Vec<CoreIndex>, DispatchError> {
		let validators = Validators::get();
		let session_index = CurrentSessionIndex::get();
		let config = <configuration::Module<T>>::config();
		let parachains = <paras::Module<T>>::parachains();

		let n_bits = parachains.len() + config.parathread_cores as usize;

		let mut assigned_paras_record: Vec<_> = (0..n_bits)
			.map(|bit_index| core_lookup(CoreIndex::from(bit_index as u32)))
			.map(|core_para| core_para.map(|p| (p, PendingAvailability::<T>::get(&p))))
			.collect();

		// do sanity checks on the bitfields:
		// 1. no more than one bitfield per validator
		// 2. bitfields are ascending by validator index.
		// 3. each bitfield has exactly `n_bits`
		// 4. signature is valid.
		{
			let occupied_bitmask: BitVec<BitOrderLsb0, u8> = assigned_paras_record.iter()
				.map(|p| p.as_ref()
					.map_or(false, |(_id, pending_availability)| pending_availability.is_some())
				)
				.collect();

			let mut last_index = None;
			let mut payload_encode_buf = Vec::new();

			let signing_context = SigningContext {
				parent_hash: <system::Module<T>>::parent_hash(),
				session_index,
			};

			for signed_bitfield in &signed_bitfields.0 {
				ensure!(
					signed_bitfield.bitfield.0.len() == n_bits,
					Error::<T>::WrongBitfieldSize,
				);

				ensure!(
					last_index.map_or(true, |last| last < signed_bitfield.validator_index),
					Error::<T>::BitfieldDuplicateOrUnordered,
				);

				ensure!(
					signed_bitfield.validator_index < validators.len() as ValidatorIndex,
					Error::<T>::ValidatorIndexOutOfBounds,
				);

				ensure!(
					occupied_bitmask.clone() & signed_bitfield.bitfield.0.clone() == signed_bitfield.bitfield.0,
					Error::<T>::UnoccupiedBitInBitfield,
				);

				let validator_public = &validators[signed_bitfield.validator_index as usize];

				if let Err(()) = primitives::parachain::check_availability_bitfield_signature(
					&signed_bitfield.bitfield,
					validator_public,
					&signed_bitfield.signature,
					&signing_context,
					Some(&mut payload_encode_buf),
				) {
					Err(Error::<T>::InvalidBitfieldSignature)?;
				}

				last_index = Some(signed_bitfield.validator_index);
				payload_encode_buf.clear();
			}
		}

		let now = <system::Module<T>>::block_number();
		for signed_bitfield in signed_bitfields.0 {
			for (bit_idx, _)
				in signed_bitfield.bitfield.0.iter().enumerate().filter(|(_, is_av)| **is_av)
			{
				let record = assigned_paras_record[bit_idx]
					.as_mut()
					.expect("validator bitfields checked not to contain bits corresponding to unoccupied cores; qed");

				// defensive check - this is constructed by loading the availability bitfield record,
				// which is always `Some` if the core is occupied - that's why we're here.
				let val_idx = signed_bitfield.validator_index as usize;
				if let Some(mut bit) = record.1.as_mut()
					.and_then(|r| r.availability_votes.get_mut(val_idx))
				{
					*bit = true;
				}
			}

			let record = AvailabilityBitfieldRecord {
				bitfield: signed_bitfield.bitfield,
				submitted_at: now,
			};

			<AvailabilityBitfields<T>>::insert(&signed_bitfield.validator_index, record);
		}

		let threshold = {
			let mut threshold = (validators.len() * 2) / 3;
			threshold += (validators.len() * 2) % 3;
			threshold
		};

		let mut freed_cores = Vec::with_capacity(n_bits);
		for (para_id, pending_availability) in assigned_paras_record.into_iter()
			.filter_map(|x| x)
			.filter_map(|(id, p)| p.map(|p| (id, p)))
		{
			if pending_availability.availability_votes.count_ones() >= threshold {
				<PendingAvailability<T>>::remove(&para_id);
				Self::enact_candidate(
					pending_availability.relay_parent_number,
					pending_availability.receipt,
				);

				freed_cores.push(pending_availability.core);
			} else {
				<PendingAvailability<T>>::insert(&para_id, &pending_availability);
			}
		}

		// TODO: pass available candidates onwards to validity module once implemented.
		// https://github.com/paritytech/polkadot/issues/1251

		Ok(freed_cores)
	}

	/// Process candidates that have been backed. Provide a set of candidates and scheduled cores.
	///
	/// Both should be sorted ascending by core index, and the candidates should be a subset of
	/// scheduled cores. If these conditions are not met, the execution of the function fails.
	pub(crate) fn process_candidates(
		candidates: Vec<BackedCandidate<T::Hash>>,
		scheduled: Vec<CoreAssignment>,
		group_validators: impl Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>,
	)
		-> Result<Vec<CoreIndex>, DispatchError>
	{
		ensure!(candidates.len() <= scheduled.len(), Error::<T>::UnscheduledCandidate);

		if scheduled.is_empty() {
			return Ok(Vec::new());
		}

		let validators = Validators::get();
		let parent_hash = <system::Module<T>>::parent_hash();
		let config = <configuration::Module<T>>::config();
		let now = <system::Module<T>>::block_number();
		let relay_parent_number = now - One::one();

		// do all checks before writing storage.
		let core_indices = {
			let mut skip = 0;
			let mut core_indices = Vec::with_capacity(candidates.len());
			let mut last_core = None;

			let mut check_assignment_in_order = |assignment: &CoreAssignment| -> DispatchResult {
				ensure!(
					last_core.map_or(true, |core| assignment.core > core),
					Error::<T>::ScheduledOutOfOrder,
				);

				last_core = Some(assignment.core);
				Ok(())
			};

			let signing_context = SigningContext {
				parent_hash,
				session_index: CurrentSessionIndex::get(),
			};

			// We combine an outer loop over candidates with an inner loop over the scheduled,
			// where each iteration of the outer loop picks up at the position
			// in scheduled just after the past iteration left off.
			//
			// If the candidates appear in the same order as they appear in `scheduled`,
			// then they should always be found. If the end of `scheduled` is reached,
			// then the candidate was either not scheduled or out-of-order.
			//
			// In the meantime, we do certain sanity checks on the candidates and on the scheduled
			// list.
			'a:
			for candidate in &candidates {
				let para_id = candidate.candidate.parachain_index;

				// we require that the candidate is in the context of the parent block.
				ensure!(
					candidate.candidate.relay_parent == parent_hash,
					Error::<T>::CandidateNotInParentContext,
				);

				let code_upgrade_allowed = <paras::Module<T>>::last_code_upgrade(para_id, true)
					.map_or(
						true,
						|last| last <= relay_parent_number &&
							relay_parent_number.saturating_sub(last) >= config.validation_upgrade_frequency,
					);

				ensure!(code_upgrade_allowed, Error::<T>::PrematureCodeUpgrade);
				ensure!(
					candidate.candidate.check_signature().is_ok(),
					Error::<T>::NotCollatorSigned,
				);

				for (i, assignment) in scheduled[skip..].iter().enumerate() {
					check_assignment_in_order(assignment)?;

					if candidate.candidate.parachain_index == assignment.para_id {
						if let Some(required_collator) = assignment.required_collator() {
							ensure!(
								required_collator == &candidate.candidate.collator,
								Error::<T>::WrongCollator,
							);
						}

						ensure!(
							<PendingAvailability<T>>::get(&assignment.para_id).is_none(),
							Error::<T>::CandidateScheduledBeforeParaFree,
						);

						// account for already skipped, and then skip this one.
						skip = i + skip + 1;

						let group_vals = group_validators(assignment.group_idx)
							.ok_or_else(|| Error::<T>::InvalidGroupIndex)?;

						// check the signatures in the backing and that it is a majority.
						{
							let maybe_amount_validated
								= primitives::parachain::check_candidate_backing(
									&candidate,
									&signing_context,
									group_vals.len(),
									|idx| group_vals.get(idx)
										.and_then(|i| validators.get(*i as usize))
										.map(|v| v.clone()),
								);

							match maybe_amount_validated {
								Ok(amount_validated) => ensure!(
									amount_validated * 2 > group_vals.len(),
									Error::<T>::InsufficientBacking,
								),
								Err(()) => { Err(Error::<T>::InvalidBacking)?; }
							}
						}

						core_indices.push(assignment.core);
						continue 'a;
					}
				}

				// end of loop reached means that the candidate didn't appear in the non-traversed
				// section of the `scheduled` slice. either it was not scheduled or didn't appear in
				// `candidates` in the correct order.
				ensure!(
					false,
					Error::<T>::UnscheduledCandidate,
				);
			};

			// check remainder of scheduled cores, if any.
			for assignment in scheduled[skip..].iter() {
				check_assignment_in_order(assignment)?;
			}

			core_indices
		};

		// one more sweep for actually writing to storage.
		for (candidate, core) in candidates.into_iter().zip(core_indices.iter().cloned()) {
			let para_id = candidate.candidate.parachain_index;

			// initialize all availability votes to 0.
			let availability_votes: BitVec<BitOrderLsb0, u8>
				= bitvec::bitvec![BitOrderLsb0, u8; 0; validators.len()];
			<PendingAvailability<T>>::insert(&para_id, CandidatePendingAvailability {
				core,
				receipt: candidate.candidate,
				availability_votes,
				relay_parent_number,
				backed_in_number: now,
			});
		}

		Ok(core_indices)
	}

	fn enact_candidate(
		relay_parent_number: T::BlockNumber,
		receipt: AbridgedCandidateReceipt<T::Hash>,
	) -> Weight {
		let commitments = receipt.commitments;
		let config = <configuration::Module<T>>::config();

		// initial weight is config read.
		let mut weight = T::DbWeight::get().reads_writes(1, 0);
		if let Some(new_code) = commitments.new_validation_code {
			weight += <paras::Module<T>>::schedule_code_upgrade(
				receipt.parachain_index,
				new_code,
				relay_parent_number + config.validation_upgrade_delay,
			);
		}

		weight + <paras::Module<T>>::note_new_head(
			receipt.parachain_index,
			receipt.head_data,
			relay_parent_number,
		)
	}

	/// Cleans up all paras pending availability that the predicate returns true for.
	///
	/// The predicate accepts the index of the core and the block number the core has been occupied
	/// since (i.e. the block number the candidate was backed at in this fork of the relay chain).
	///
	/// Returns a vector of cleaned-up core IDs.
	pub(crate) fn collect_pending(pred: impl Fn(CoreIndex, T::BlockNumber) -> bool) -> Vec<CoreIndex> {
		let mut cleaned_up_ids = Vec::new();
		let mut cleaned_up_cores = Vec::new();

		for (para_id, pending_record) in <PendingAvailability<T>>::iter() {
			if pred(pending_record.core, pending_record.backed_in_number) {
				cleaned_up_ids.push(para_id);
				cleaned_up_cores.push(pending_record.core);
			}
		}

		for para_id in cleaned_up_ids {
			<PendingAvailability<T>>::remove(&para_id);
		}

		cleaned_up_cores
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use primitives::{BlockNumber, Hash};
	use primitives::parachain::{SignedAvailabilityBitfield, Statement, ValidityAttestation};
	use frame_support::traits::{OnFinalize, OnInitialize};
	use keyring::Sr25519Keyring;

	use crate::mock::{
		new_test_ext, Configuration, Paras, System, Inclusion,
		GenesisConfig as MockGenesisConfig, Test,
	};
	use crate::initializer::SessionChangeNotification;
	use crate::configuration::HostConfiguration;
	use crate::paras::ParaGenesisArgs;

	fn default_config() -> HostConfiguration<BlockNumber> {
		let mut config = HostConfiguration::default();
		config.parathread_cores = 1;
		config
	}

	fn genesis_config(paras: Vec<(ParaId, bool)>) -> MockGenesisConfig {
		MockGenesisConfig {
			paras: paras::GenesisConfig {
				paras: paras.into_iter().map(|(id, is_chain)| (id, ParaGenesisArgs {
					genesis_head: Vec::new().into(),
					validation_code: Vec::new().into(),
					parachain: is_chain,
				})).collect(),
				..Default::default()
			},
			configuration: configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		}
	}

	#[derive(Clone, Copy, PartialEq)]
	enum BackingKind {
		Unanimous,
		Threshold,
		Lacking,
	}

	fn collator_sign_candidate(
		collator: Sr25519Keyring,
		candidate: &mut AbridgedCandidateReceipt,
	) {
		candidate.collator = collator.public().into();

		let payload = primitives::parachain::collator_signature_payload(
			&candidate.relay_parent,
			&candidate.parachain_index,
			&candidate.pov_block_hash,
		);

		candidate.signature = collator.sign(&payload[..]).into();
		assert!(candidate.check_signature().is_ok());
	}

	fn back_candidate(
		candidate: AbridgedCandidateReceipt,
		validators: &[Sr25519Keyring],
		group: &[ValidatorIndex],
		signing_context: &SigningContext,
		kind: BackingKind,
	) -> BackedCandidate {
		let mut validator_indices = bitvec::bitvec![BitOrderLsb0, u8; 0; group.len()];
		let threshold = (group.len() / 2) + 1;

		let signing = match kind {
			BackingKind::Unanimous => group.len(),
			BackingKind::Threshold => threshold,
			BackingKind::Lacking => threshold.saturating_sub(1),
		};

		let mut validity_votes = Vec::with_capacity(signing);
		let candidate_hash = candidate.hash();
		let payload = Statement::Valid(candidate_hash).signing_payload(signing_context);

		for (idx_in_group, val_idx) in group.iter().enumerate().take(signing) {
			let key: Sr25519Keyring = validators[*val_idx as usize];
			*validator_indices.get_mut(idx_in_group).unwrap() = true;

			validity_votes.push(ValidityAttestation::Explicit(key.sign(&payload[..]).into()));
		}

		let backed = BackedCandidate {
			candidate,
			validity_votes,
			validator_indices,
		};

		let should_pass = match kind {
			BackingKind::Unanimous | BackingKind::Threshold => true,
			BackingKind::Lacking => false,
		};

		assert_eq!(
			primitives::parachain::check_candidate_backing(
				&backed,
				signing_context,
				group.len(),
				|i| Some(validators[i].public().into()),
			).is_ok(),
			should_pass,
		);

		backed
	}

	fn run_to_block(
		to: BlockNumber,
		new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
	) {
		while System::block_number() < to {
			let b = System::block_number();

			Inclusion::initializer_finalize();
			Paras::initializer_finalize();

			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			if let Some(notification) = new_session(b + 1) {
				Paras::initializer_on_new_session(&notification);
				Inclusion::initializer_on_new_session(&notification);
			}

			Paras::initializer_initialize(b + 1);
			Inclusion::initializer_initialize(b + 1);
		}
	}

	fn default_bitfield() -> AvailabilityBitfield {
		let n_bits = Paras::parachains().len() + Configuration::config().parathread_cores as usize;

		AvailabilityBitfield(bitvec::bitvec![BitOrderLsb0, u8; 0; n_bits])
	}

	fn default_availability_votes() -> BitVec<BitOrderLsb0, u8> {
		bitvec::bitvec![BitOrderLsb0, u8; 0; Validators::get().len()]
	}

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	fn sign_bitfield(
		key: &Sr25519Keyring,
		validator_index: ValidatorIndex,
		bitfield: AvailabilityBitfield,
		signing_context: &SigningContext,
	)
		-> SignedAvailabilityBitfield
	{
		let payload = bitfield.encode_signing_payload(signing_context);

		SignedAvailabilityBitfield {
			validator_index,
			bitfield: bitfield,
			signature: key.sign(&payload[..]).into(),
		}
	}

	#[test]
	fn collect_pending_cleans_up_pending() {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);
		let thread_a = ParaId::from(3);

		let paras = vec![(chain_a, true), (chain_b, true), (thread_a, false)];
		new_test_ext(genesis_config(paras)).execute_with(|| {
			<PendingAvailability<Test>>::insert(chain_a, CandidatePendingAvailability {
				core: CoreIndex::from(0),
				receipt: Default::default(),
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
			});

			<PendingAvailability<Test>>::insert(chain_b, CandidatePendingAvailability {
				core: CoreIndex::from(1),
				receipt: Default::default(),
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
			});

			run_to_block(5, |_| None);

			assert!(<PendingAvailability<Test>>::get(&chain_a).is_some());
			assert!(<PendingAvailability<Test>>::get(&chain_b).is_some());

			Inclusion::collect_pending(|core, _since| core == CoreIndex::from(0));

			assert!(<PendingAvailability<Test>>::get(&chain_a).is_none());
			assert!(<PendingAvailability<Test>>::get(&chain_b).is_some());
		});
	}

	#[test]
	fn bitfield_checks() {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);
		let thread_a = ParaId::from(3);

		let paras = vec![(chain_a, true), (chain_b, true), (thread_a, false)];
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];
		let validator_public = validator_pubkeys(&validators);

		new_test_ext(genesis_config(paras)).execute_with(|| {
			Validators::set(validator_public.clone());
			CurrentSessionIndex::set(5);

			let signing_context = SigningContext {
				parent_hash: System::parent_hash(),
				session_index: 5,
			};

			let core_lookup = |core| match core {
				core if core == CoreIndex::from(0) => Some(chain_a),
				core if core == CoreIndex::from(1) => Some(chain_b),
				core if core == CoreIndex::from(2) => Some(thread_a),
				_ => panic!("Core out of bounds for 2 parachains and 1 parathread core."),
			};

			// wrong number of bits.
			{
				let mut bare_bitfield = default_bitfield();
				bare_bitfield.0.push(false);
				let signed = sign_bitfield(
					&validators[0],
					0,
					bare_bitfield,
					&signing_context,
				);

				assert!(Inclusion::process_bitfields(
					SignedAvailabilityBitfields(vec![signed]),
					&core_lookup,
				).is_err());
			}

			// duplicate.
			{
				let bare_bitfield = default_bitfield();
				let signed = sign_bitfield(
					&validators[0],
					0,
					bare_bitfield,
					&signing_context,
				);

				assert!(Inclusion::process_bitfields(
					SignedAvailabilityBitfields(vec![signed.clone(), signed]),
					&core_lookup,
				).is_err());
			}

			// out of order.
			{
				let bare_bitfield = default_bitfield();
				let signed_0 = sign_bitfield(
					&validators[0],
					0,
					bare_bitfield.clone(),
					&signing_context,
				);

				let signed_1 = sign_bitfield(
					&validators[1],
					1,
					bare_bitfield,
					&signing_context,
				);

				assert!(Inclusion::process_bitfields(
					SignedAvailabilityBitfields(vec![signed_1, signed_0]),
					&core_lookup,
				).is_err());
			}

			// non-pending bit set.
			{
				let mut bare_bitfield = default_bitfield();
				*bare_bitfield.0.get_mut(0).unwrap() = true;
				let signed = sign_bitfield(
					&validators[0],
					0,
					bare_bitfield,
					&signing_context,
				);

				assert!(Inclusion::process_bitfields(
					SignedAvailabilityBitfields(vec![signed]),
					&core_lookup,
				).is_err());
			}

			// empty bitfield signed: always OK, but kind of useless.
			{
				let bare_bitfield = default_bitfield();
				let signed = sign_bitfield(
					&validators[0],
					0,
					bare_bitfield,
					&signing_context,
				);

				assert!(Inclusion::process_bitfields(
					SignedAvailabilityBitfields(vec![signed]),
					&core_lookup,
				).is_ok());
			}

			// bitfield signed with pending bit signed.
			{
				let mut bare_bitfield = default_bitfield();

				assert_eq!(core_lookup(CoreIndex::from(0)), Some(chain_a));

				<PendingAvailability<Test>>::insert(chain_a, CandidatePendingAvailability {
					core: CoreIndex::from(0),
					receipt: Default::default(),
					availability_votes: default_availability_votes(),
					relay_parent_number: 0,
					backed_in_number: 0,
				});

				*bare_bitfield.0.get_mut(0).unwrap() = true;
				let signed = sign_bitfield(
					&validators[0],
					0,
					bare_bitfield,
					&signing_context,
				);

				assert!(Inclusion::process_bitfields(
					SignedAvailabilityBitfields(vec![signed]),
					&core_lookup,
				).is_ok());
			}
		});
	}

	#[test]
	fn supermajority_bitfields_trigger_availability() {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);
		let thread_a = ParaId::from(3);

		let paras = vec![(chain_a, true), (chain_b, true), (thread_a, false)];
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];
		let validator_public = validator_pubkeys(&validators);

		new_test_ext(genesis_config(paras)).execute_with(|| {
			Validators::set(validator_public.clone());
			CurrentSessionIndex::set(5);

			let signing_context = SigningContext {
				parent_hash: System::parent_hash(),
				session_index: 5,
			};

			let core_lookup = |core| match core {
				core if core == CoreIndex::from(0) => Some(chain_a),
				core if core == CoreIndex::from(1) => Some(chain_b),
				core if core == CoreIndex::from(2) => Some(thread_a),
				_ => panic!("Core out of bounds for 2 parachains and 1 parathread core."),
			};

			<PendingAvailability<Test>>::insert(chain_a, CandidatePendingAvailability {
				core: CoreIndex::from(0),
				receipt: AbridgedCandidateReceipt {
					parachain_index: chain_a,
					head_data: vec![1, 2, 3, 4].into(),
					..Default::default()
				},
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
			});

			<PendingAvailability<Test>>::insert(chain_b, CandidatePendingAvailability {
				core: CoreIndex::from(1),
				receipt: AbridgedCandidateReceipt {
					parachain_index: chain_b,
					head_data: vec![5, 6, 7, 8].into(),
					..Default::default()
				},
				availability_votes: default_availability_votes(),
				relay_parent_number: 0,
				backed_in_number: 0,
			});

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

			let threshold = {
				let mut threshold = (validators.len() * 2) / 3;
				threshold += (validators.len() * 2) % 3;
				threshold
			};

			// 4 of 5 first value >= 2/3
			assert_eq!(threshold, 4);

			let signed_bitfields = validators.iter().enumerate().filter_map(|(i, key)| {
				let to_sign = if i < 3 {
					a_and_b_available.clone()
				} else if i < 4 {
					a_available.clone()
				} else {
					// sign nothing.
					return None
				};

				Some(sign_bitfield(
					key,
					i as ValidatorIndex,
					to_sign,
					&signing_context,
				))
			}).collect();

			assert!(Inclusion::process_bitfields(
				SignedAvailabilityBitfields(signed_bitfields),
				&core_lookup,
			).is_ok());

			// chain A had 4 signing off, which is >= threshold.
			// chain B has 3 signing off, which is < threshold.
			assert!(<PendingAvailability<Test>>::get(&chain_a).is_none());
			assert_eq!(
				<PendingAvailability<Test>>::get(&chain_b).unwrap().availability_votes,
				{
					// check that votes from first 3 were tracked.

					let mut votes = default_availability_votes();
					*votes.get_mut(0).unwrap() = true;
					*votes.get_mut(1).unwrap() = true;
					*votes.get_mut(2).unwrap() = true;

					votes
				},
			);

			// and check that chain head was enacted.
			assert_eq!(Paras::para_head(&chain_a), Some(vec![1, 2, 3, 4].into()));
		});
	}

	#[test]
	fn candidate_checks() {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);
		let thread_a = ParaId::from(3);

		let paras = vec![(chain_a, true), (chain_b, true), (thread_a, false)];
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];
		let validator_public = validator_pubkeys(&validators);

		new_test_ext(genesis_config(paras)).execute_with(|| {
			Validators::set(validator_public.clone());
			CurrentSessionIndex::set(5);

			run_to_block(5, |_| None);

			let signing_context = SigningContext {
				parent_hash: System::parent_hash(),
				session_index: 5,
			};

			let group_validators = |group_index: GroupIndex| match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1]),
				group_index if group_index == GroupIndex::from(1) => Some(vec![3, 4]),
				group_index if group_index == GroupIndex::from(2) => Some(vec![5]),
				_ => panic!("Group index out of bounds for 2 parachains and 1 parathread core"),
			};

			// unscheduled candidate.
			{
				let mut candidate = AbridgedCandidateReceipt {
					parachain_index: chain_a,
					relay_parent: System::parent_hash(),
					pov_block_hash: Hash::from([1; 32]),
					..Default::default()
				};
				collator_sign_candidate(
					Sr25519Keyring::One,
					&mut candidate,
				);
			}

			// candidates out of order.
			{

			}

			// candidate not backed.
			{

			}

			// candidate not in parent context.
			{

			}

			// candidate has wrong collator.
			{

			}
		});
	}

	// TODO [now]: candidate with interfering code upgrade is rejected.
	// TODO [now]: session change wipes everything and updates validators / session index.
}
