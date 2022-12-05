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

//! Assignment criteria VRF generation and checking.

use parity_scale_codec::{Decode, Encode};
use polkadot_node_primitives::approval::{
	self as approval_types, AssignmentCert, AssignmentCertKind, DelayTranche, RelayVRFStory,
};
use polkadot_primitives::v2::{
	AssignmentId, AssignmentPair, CandidateHash, CoreIndex, GroupIndex, IndexedVec, SessionInfo,
	ValidatorIndex,
};
use sc_keystore::LocalKeystore;
use sp_application_crypto::ByteArray;

use merlin::Transcript;
use schnorrkel::vrf::VRFInOut;

use std::collections::{hash_map::Entry, HashMap};

use super::LOG_TARGET;

/// Details pertaining to our assignment on a block.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub struct OurAssignment {
	cert: AssignmentCert,
	tranche: DelayTranche,
	validator_index: ValidatorIndex,
	// Whether the assignment has been triggered already.
	triggered: bool,
}

impl OurAssignment {
	pub(crate) fn cert(&self) -> &AssignmentCert {
		&self.cert
	}

	pub(crate) fn tranche(&self) -> DelayTranche {
		self.tranche
	}

	pub(crate) fn validator_index(&self) -> ValidatorIndex {
		self.validator_index
	}

	pub(crate) fn triggered(&self) -> bool {
		self.triggered
	}

	pub(crate) fn mark_triggered(&mut self) {
		self.triggered = true;
	}
}

impl From<crate::approval_db::v1::OurAssignment> for OurAssignment {
	fn from(entry: crate::approval_db::v1::OurAssignment) -> Self {
		OurAssignment {
			cert: entry.cert,
			tranche: entry.tranche,
			validator_index: entry.validator_index,
			triggered: entry.triggered,
		}
	}
}

impl From<OurAssignment> for crate::approval_db::v1::OurAssignment {
	fn from(entry: OurAssignment) -> Self {
		Self {
			cert: entry.cert,
			tranche: entry.tranche,
			validator_index: entry.validator_index,
			triggered: entry.triggered,
		}
	}
}

fn relay_vrf_modulo_transcript(relay_vrf_story: RelayVRFStory, sample: u32) -> Transcript {
	// combine the relay VRF story with a sample number.
	let mut t = Transcript::new(approval_types::RELAY_VRF_MODULO_CONTEXT);
	t.append_message(b"RC-VRF", &relay_vrf_story.0);
	sample.using_encoded(|s| t.append_message(b"sample", s));

	t
}

fn relay_vrf_modulo_core(vrf_in_out: &VRFInOut, n_cores: u32) -> CoreIndex {
	let bytes: [u8; 4] = vrf_in_out.make_bytes(approval_types::CORE_RANDOMNESS_CONTEXT);

	// interpret as little-endian u32.
	let random_core = u32::from_le_bytes(bytes) % n_cores;
	CoreIndex(random_core)
}

fn relay_vrf_delay_transcript(relay_vrf_story: RelayVRFStory, core_index: CoreIndex) -> Transcript {
	let mut t = Transcript::new(approval_types::RELAY_VRF_DELAY_CONTEXT);
	t.append_message(b"RC-VRF", &relay_vrf_story.0);
	core_index.0.using_encoded(|s| t.append_message(b"core", s));
	t
}

fn relay_vrf_delay_tranche(
	vrf_in_out: &VRFInOut,
	num_delay_tranches: u32,
	zeroth_delay_tranche_width: u32,
) -> DelayTranche {
	let bytes: [u8; 4] = vrf_in_out.make_bytes(approval_types::TRANCHE_RANDOMNESS_CONTEXT);

	// interpret as little-endian u32 and reduce by the number of tranches.
	let wide_tranche =
		u32::from_le_bytes(bytes) % (num_delay_tranches + zeroth_delay_tranche_width);

	// Consolidate early results to tranche zero so tranche zero is extra wide.
	wide_tranche.saturating_sub(zeroth_delay_tranche_width)
}

fn assigned_core_transcript(core_index: CoreIndex) -> Transcript {
	let mut t = Transcript::new(approval_types::ASSIGNED_CORE_CONTEXT);
	core_index.0.using_encoded(|s| t.append_message(b"core", s));
	t
}

/// Information about the world assignments are being produced in.
#[derive(Clone)]
pub(crate) struct Config {
	/// The assignment public keys for validators.
	assignment_keys: Vec<AssignmentId>,
	/// The groups of validators assigned to each core.
	validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	/// The number of availability cores used by the protocol during this session.
	n_cores: u32,
	/// The zeroth delay tranche width.
	zeroth_delay_tranche_width: u32,
	/// The number of samples we do of `relay_vrf_modulo`.
	relay_vrf_modulo_samples: u32,
	/// The number of delay tranches in total.
	n_delay_tranches: u32,
}

impl<'a> From<&'a SessionInfo> for Config {
	fn from(s: &'a SessionInfo) -> Self {
		Config {
			assignment_keys: s.assignment_keys.clone(),
			validator_groups: s.validator_groups.clone(),
			n_cores: s.n_cores,
			zeroth_delay_tranche_width: s.zeroth_delay_tranche_width,
			relay_vrf_modulo_samples: s.relay_vrf_modulo_samples,
			n_delay_tranches: s.n_delay_tranches,
		}
	}
}

/// A trait for producing and checking assignments. Used to mock.
pub(crate) trait AssignmentCriteria {
	fn compute_assignments(
		&self,
		keystore: &LocalKeystore,
		relay_vrf_story: RelayVRFStory,
		config: &Config,
		leaving_cores: Vec<(CandidateHash, CoreIndex, GroupIndex)>,
	) -> HashMap<CoreIndex, OurAssignment>;

	fn check_assignment_cert(
		&self,
		claimed_core_index: CoreIndex,
		validator_index: ValidatorIndex,
		config: &Config,
		relay_vrf_story: RelayVRFStory,
		assignment: &AssignmentCert,
		backing_group: GroupIndex,
	) -> Result<DelayTranche, InvalidAssignment>;
}

pub(crate) struct RealAssignmentCriteria;

impl AssignmentCriteria for RealAssignmentCriteria {
	fn compute_assignments(
		&self,
		keystore: &LocalKeystore,
		relay_vrf_story: RelayVRFStory,
		config: &Config,
		leaving_cores: Vec<(CandidateHash, CoreIndex, GroupIndex)>,
	) -> HashMap<CoreIndex, OurAssignment> {
		compute_assignments(keystore, relay_vrf_story, config, leaving_cores)
	}

	fn check_assignment_cert(
		&self,
		claimed_core_index: CoreIndex,
		validator_index: ValidatorIndex,
		config: &Config,
		relay_vrf_story: RelayVRFStory,
		assignment: &AssignmentCert,
		backing_group: GroupIndex,
	) -> Result<DelayTranche, InvalidAssignment> {
		check_assignment_cert(
			claimed_core_index,
			validator_index,
			config,
			relay_vrf_story,
			assignment,
			backing_group,
		)
	}
}

/// Compute the assignments for a given block. Returns a map containing all assignments to cores in
/// the block. If more than one assignment targets the given core, only the earliest assignment is kept.
///
/// The `leaving_cores` parameter indicates all cores within the block where a candidate was included,
/// as well as the group index backing those.
///
/// The current description of the protocol assigns every validator to check every core. But at different times.
/// The idea is that most assignments are never triggered and fall by the wayside.
///
/// This will not assign to anything the local validator was part of the backing group for.
pub(crate) fn compute_assignments(
	keystore: &LocalKeystore,
	relay_vrf_story: RelayVRFStory,
	config: &Config,
	leaving_cores: impl IntoIterator<Item = (CandidateHash, CoreIndex, GroupIndex)> + Clone,
) -> HashMap<CoreIndex, OurAssignment> {
	if config.n_cores == 0 ||
		config.assignment_keys.is_empty() ||
		config.validator_groups.is_empty()
	{
		gum::trace!(
			target: LOG_TARGET,
			n_cores = config.n_cores,
			has_assignment_keys = !config.assignment_keys.is_empty(),
			has_validator_groups = !config.validator_groups.is_empty(),
			"Not producing assignments because config is degenerate",
		);

		return HashMap::new()
	}

	let (index, assignments_key): (ValidatorIndex, AssignmentPair) = {
		let key = config.assignment_keys.iter().enumerate().find_map(|(i, p)| {
			match keystore.key_pair(p) {
				Ok(Some(pair)) => Some((ValidatorIndex(i as _), pair)),
				Ok(None) => None,
				Err(sc_keystore::Error::Unavailable) => None,
				Err(sc_keystore::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
				Err(e) => {
					gum::warn!(target: LOG_TARGET, "Encountered keystore error: {:?}", e);
					None
				},
			}
		});

		match key {
			None => {
				gum::trace!(target: LOG_TARGET, "No assignment key");
				return HashMap::new()
			},
			Some(k) => k,
		}
	};

	// Ignore any cores where the assigned group is our own.
	let leaving_cores = leaving_cores
		.into_iter()
		.filter(|&(_, _, ref g)| !is_in_backing_group(&config.validator_groups, index, *g))
		.map(|(c_hash, core, _)| (c_hash, core))
		.collect::<Vec<_>>();

	gum::trace!(
		target: LOG_TARGET,
		assignable_cores = leaving_cores.len(),
		"Assigning to candidates from different backing groups"
	);

	let assignments_key: &sp_application_crypto::sr25519::Pair = assignments_key.as_ref();
	let assignments_key: &schnorrkel::Keypair = assignments_key.as_ref();

	let mut assignments = HashMap::new();

	// First run `RelayVRFModulo` for each sample.
	compute_relay_vrf_modulo_assignments(
		&assignments_key,
		index,
		config,
		relay_vrf_story.clone(),
		leaving_cores.iter().cloned(),
		&mut assignments,
	);

	// Then run `RelayVRFDelay` once for the whole block.
	compute_relay_vrf_delay_assignments(
		&assignments_key,
		index,
		config,
		relay_vrf_story,
		leaving_cores,
		&mut assignments,
	);

	assignments
}

fn compute_relay_vrf_modulo_assignments(
	assignments_key: &schnorrkel::Keypair,
	validator_index: ValidatorIndex,
	config: &Config,
	relay_vrf_story: RelayVRFStory,
	leaving_cores: impl IntoIterator<Item = (CandidateHash, CoreIndex)> + Clone,
	assignments: &mut HashMap<CoreIndex, OurAssignment>,
) {
	for rvm_sample in 0..config.relay_vrf_modulo_samples {
		let mut core = CoreIndex::default();

		let maybe_assignment = {
			// Extra scope to ensure borrowing instead of moving core
			// into closure.
			let core = &mut core;
			assignments_key.vrf_sign_extra_after_check(
				relay_vrf_modulo_transcript(relay_vrf_story.clone(), rvm_sample),
				|vrf_in_out| {
					*core = relay_vrf_modulo_core(&vrf_in_out, config.n_cores);
					if let Some((candidate_hash, _)) =
						leaving_cores.clone().into_iter().find(|(_, c)| c == core)
					{
						gum::trace!(
							target: LOG_TARGET,
							?candidate_hash,
							?core,
							?validator_index,
							tranche = 0,
							"RelayVRFModulo Assignment."
						);

						Some(assigned_core_transcript(*core))
					} else {
						None
					}
				},
			)
		};

		if let Some((vrf_in_out, vrf_proof, _)) = maybe_assignment {
			// Sanity: `core` is always initialized to non-default here, as the closure above
			// has been executed.
			let cert = AssignmentCert {
				kind: AssignmentCertKind::RelayVRFModulo { sample: rvm_sample },
				vrf: (
					approval_types::VRFOutput(vrf_in_out.to_output()),
					approval_types::VRFProof(vrf_proof),
				),
			};

			// All assignments of type RelayVRFModulo have tranche 0.
			assignments.entry(core).or_insert(OurAssignment {
				cert,
				tranche: 0,
				validator_index,
				triggered: false,
			});
		}
	}
}

fn compute_relay_vrf_delay_assignments(
	assignments_key: &schnorrkel::Keypair,
	validator_index: ValidatorIndex,
	config: &Config,
	relay_vrf_story: RelayVRFStory,
	leaving_cores: impl IntoIterator<Item = (CandidateHash, CoreIndex)>,
	assignments: &mut HashMap<CoreIndex, OurAssignment>,
) {
	for (candidate_hash, core) in leaving_cores {
		let (vrf_in_out, vrf_proof, _) =
			assignments_key.vrf_sign(relay_vrf_delay_transcript(relay_vrf_story.clone(), core));

		let tranche = relay_vrf_delay_tranche(
			&vrf_in_out,
			config.n_delay_tranches,
			config.zeroth_delay_tranche_width,
		);

		let cert = AssignmentCert {
			kind: AssignmentCertKind::RelayVRFDelay { core_index: core },
			vrf: (
				approval_types::VRFOutput(vrf_in_out.to_output()),
				approval_types::VRFProof(vrf_proof),
			),
		};

		let our_assignment = OurAssignment { cert, tranche, validator_index, triggered: false };

		let used = match assignments.entry(core) {
			Entry::Vacant(e) => {
				let _ = e.insert(our_assignment);
				true
			},
			Entry::Occupied(mut e) =>
				if e.get().tranche > our_assignment.tranche {
					e.insert(our_assignment);
					true
				} else {
					false
				},
		};

		if used {
			gum::trace!(
				target: LOG_TARGET,
				?candidate_hash,
				?core,
				?validator_index,
				tranche,
				"RelayVRFDelay Assignment",
			);
		}
	}
}

/// Assignment invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAssignment(pub(crate) InvalidAssignmentReason);

impl std::fmt::Display for InvalidAssignment {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "Invalid Assignment: {:?}", self.0)
	}
}

impl std::error::Error for InvalidAssignment {}

/// Failure conditions when checking an assignment cert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvalidAssignmentReason {
	ValidatorIndexOutOfBounds,
	SampleOutOfBounds,
	CoreIndexOutOfBounds,
	InvalidAssignmentKey,
	IsInBackingGroup,
	VRFModuloCoreIndexMismatch,
	VRFModuloOutputMismatch,
	VRFDelayCoreIndexMismatch,
	VRFDelayOutputMismatch,
}

/// Checks the crypto of an assignment cert. Failure conditions:
///   * Validator index out of bounds
///   * VRF signature check fails
///   * VRF output doesn't match assigned core
///   * Core is not covered by extra data in signature
///   * Core index out of bounds
///   * Sample is out of bounds
///   * Validator is present in backing group.
///
/// This function does not check whether the core is actually a valid assignment or not. That should be done
/// outside the scope of this function.
pub(crate) fn check_assignment_cert(
	claimed_core_index: CoreIndex,
	validator_index: ValidatorIndex,
	config: &Config,
	relay_vrf_story: RelayVRFStory,
	assignment: &AssignmentCert,
	backing_group: GroupIndex,
) -> Result<DelayTranche, InvalidAssignment> {
	use InvalidAssignmentReason as Reason;

	let validator_public = config
		.assignment_keys
		.get(validator_index.0 as usize)
		.ok_or(InvalidAssignment(Reason::ValidatorIndexOutOfBounds))?;

	let public = schnorrkel::PublicKey::from_bytes(validator_public.as_slice())
		.map_err(|_| InvalidAssignment(Reason::InvalidAssignmentKey))?;

	if claimed_core_index.0 >= config.n_cores {
		return Err(InvalidAssignment(Reason::CoreIndexOutOfBounds))
	}

	// Check that the validator was not part of the backing group
	// and not already assigned.
	let is_in_backing =
		is_in_backing_group(&config.validator_groups, validator_index, backing_group);

	if is_in_backing {
		return Err(InvalidAssignment(Reason::IsInBackingGroup))
	}

	let &(ref vrf_output, ref vrf_proof) = &assignment.vrf;
	match assignment.kind {
		AssignmentCertKind::RelayVRFModulo { sample } => {
			if sample >= config.relay_vrf_modulo_samples {
				return Err(InvalidAssignment(Reason::SampleOutOfBounds))
			}

			let (vrf_in_out, _) = public
				.vrf_verify_extra(
					relay_vrf_modulo_transcript(relay_vrf_story, sample),
					&vrf_output.0,
					&vrf_proof.0,
					assigned_core_transcript(claimed_core_index),
				)
				.map_err(|_| InvalidAssignment(Reason::VRFModuloOutputMismatch))?;

			// ensure that the `vrf_in_out` actually gives us the claimed core.
			if relay_vrf_modulo_core(&vrf_in_out, config.n_cores) == claimed_core_index {
				Ok(0)
			} else {
				Err(InvalidAssignment(Reason::VRFModuloCoreIndexMismatch))
			}
		},
		AssignmentCertKind::RelayVRFDelay { core_index } => {
			if core_index != claimed_core_index {
				return Err(InvalidAssignment(Reason::VRFDelayCoreIndexMismatch))
			}

			let (vrf_in_out, _) = public
				.vrf_verify(
					relay_vrf_delay_transcript(relay_vrf_story, core_index),
					&vrf_output.0,
					&vrf_proof.0,
				)
				.map_err(|_| InvalidAssignment(Reason::VRFDelayOutputMismatch))?;

			Ok(relay_vrf_delay_tranche(
				&vrf_in_out,
				config.n_delay_tranches,
				config.zeroth_delay_tranche_width,
			))
		},
	}
}

fn is_in_backing_group(
	validator_groups: &IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	validator: ValidatorIndex,
	group: GroupIndex,
) -> bool {
	validator_groups.get(group).map_or(false, |g| g.contains(&validator))
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_node_primitives::approval::{VRFOutput, VRFProof};
	use polkadot_primitives::v2::{Hash, ASSIGNMENT_KEY_TYPE_ID};
	use sp_application_crypto::sr25519;
	use sp_core::crypto::Pair as PairT;
	use sp_keyring::sr25519::Keyring as Sr25519Keyring;
	use sp_keystore::CryptoStore;

	// sets up a keystore with the given keyring accounts.
	async fn make_keystore(accounts: &[Sr25519Keyring]) -> LocalKeystore {
		let store = LocalKeystore::in_memory();

		for s in accounts.iter().copied().map(|k| k.to_seed()) {
			store
				.sr25519_generate_new(ASSIGNMENT_KEY_TYPE_ID, Some(s.as_str()))
				.await
				.unwrap();
		}

		store
	}

	fn assignment_keys(accounts: &[Sr25519Keyring]) -> Vec<AssignmentId> {
		assignment_keys_plus_random(accounts, 0)
	}

	fn assignment_keys_plus_random(
		accounts: &[Sr25519Keyring],
		random: usize,
	) -> Vec<AssignmentId> {
		let gen_random =
			(0..random).map(|_| AssignmentId::from(sr25519::Pair::generate().0.public()));

		accounts
			.iter()
			.map(|k| AssignmentId::from(k.public()))
			.chain(gen_random)
			.collect()
	}

	fn basic_groups(
		n_validators: usize,
		n_groups: usize,
	) -> IndexedVec<GroupIndex, Vec<ValidatorIndex>> {
		let size = n_validators / n_groups;
		let big_groups = n_validators % n_groups;
		let scraps = n_groups * size;

		(0..n_groups)
			.map(|i| {
				(i * size..(i + 1) * size)
					.chain(if i < big_groups { Some(scraps + i) } else { None })
					.map(|j| ValidatorIndex(j as _))
					.collect::<Vec<_>>()
			})
			.collect()
	}

	// used for generating assignments where the validity of the VRF doesn't matter.
	fn garbage_vrf() -> (VRFOutput, VRFProof) {
		let key = Sr25519Keyring::Alice.pair();
		let key: &schnorrkel::Keypair = key.as_ref();

		let (o, p, _) = key.vrf_sign(Transcript::new(b"test-garbage"));
		(VRFOutput(o.to_output()), VRFProof(p))
	}

	#[test]
	fn assignments_produced_for_non_backing() {
		let keystore = futures::executor::block_on(make_keystore(&[Sr25519Keyring::Alice]));

		let c_a = CandidateHash(Hash::repeat_byte(0));
		let c_b = CandidateHash(Hash::repeat_byte(1));

		let relay_vrf_story = RelayVRFStory([42u8; 32]);
		let assignments = compute_assignments(
			&keystore,
			relay_vrf_story,
			&Config {
				assignment_keys: assignment_keys(&[
					Sr25519Keyring::Alice,
					Sr25519Keyring::Bob,
					Sr25519Keyring::Charlie,
				]),
				validator_groups: IndexedVec::<GroupIndex, Vec<ValidatorIndex>>::from(vec![
					vec![ValidatorIndex(0)],
					vec![ValidatorIndex(1), ValidatorIndex(2)],
				]),
				n_cores: 2,
				zeroth_delay_tranche_width: 10,
				relay_vrf_modulo_samples: 3,
				n_delay_tranches: 40,
			},
			vec![(c_a, CoreIndex(0), GroupIndex(1)), (c_b, CoreIndex(1), GroupIndex(0))],
		);

		// Note that alice is in group 0, which was the backing group for core 1.
		// Alice should have self-assigned to check core 0 but not 1.
		assert_eq!(assignments.len(), 1);
		assert!(assignments.get(&CoreIndex(0)).is_some());
	}

	#[test]
	fn assign_to_nonzero_core() {
		let keystore = futures::executor::block_on(make_keystore(&[Sr25519Keyring::Alice]));

		let c_a = CandidateHash(Hash::repeat_byte(0));
		let c_b = CandidateHash(Hash::repeat_byte(1));

		let relay_vrf_story = RelayVRFStory([42u8; 32]);
		let assignments = compute_assignments(
			&keystore,
			relay_vrf_story,
			&Config {
				assignment_keys: assignment_keys(&[
					Sr25519Keyring::Alice,
					Sr25519Keyring::Bob,
					Sr25519Keyring::Charlie,
				]),
				validator_groups: IndexedVec::<GroupIndex, Vec<ValidatorIndex>>::from(vec![
					vec![ValidatorIndex(0)],
					vec![ValidatorIndex(1), ValidatorIndex(2)],
				]),
				n_cores: 2,
				zeroth_delay_tranche_width: 10,
				relay_vrf_modulo_samples: 3,
				n_delay_tranches: 40,
			},
			vec![(c_a, CoreIndex(0), GroupIndex(0)), (c_b, CoreIndex(1), GroupIndex(1))],
		);

		assert_eq!(assignments.len(), 1);
		assert!(assignments.get(&CoreIndex(1)).is_some());
	}

	#[test]
	fn succeeds_empty_for_0_cores() {
		let keystore = futures::executor::block_on(make_keystore(&[Sr25519Keyring::Alice]));

		let relay_vrf_story = RelayVRFStory([42u8; 32]);
		let assignments = compute_assignments(
			&keystore,
			relay_vrf_story,
			&Config {
				assignment_keys: assignment_keys(&[
					Sr25519Keyring::Alice,
					Sr25519Keyring::Bob,
					Sr25519Keyring::Charlie,
				]),
				validator_groups: Default::default(),
				n_cores: 0,
				zeroth_delay_tranche_width: 10,
				relay_vrf_modulo_samples: 3,
				n_delay_tranches: 40,
			},
			vec![],
		);

		assert!(assignments.is_empty());
	}

	struct MutatedAssignment {
		core: CoreIndex,
		cert: AssignmentCert,
		group: GroupIndex,
		own_group: GroupIndex,
		val_index: ValidatorIndex,
		config: Config,
	}

	// This fails if the closure requests to skip everything.
	fn check_mutated_assignments(
		n_validators: usize,
		n_cores: usize,
		rotation_offset: usize,
		f: impl Fn(&mut MutatedAssignment) -> Option<bool>, // None = skip
	) {
		let keystore = futures::executor::block_on(make_keystore(&[Sr25519Keyring::Alice]));

		let group_for_core = |i| GroupIndex(((i + rotation_offset) % n_cores) as _);

		let config = Config {
			assignment_keys: assignment_keys_plus_random(
				&[Sr25519Keyring::Alice],
				n_validators - 1,
			),
			validator_groups: basic_groups(n_validators, n_cores),
			n_cores: n_cores as u32,
			zeroth_delay_tranche_width: 10,
			relay_vrf_modulo_samples: 3,
			n_delay_tranches: 40,
		};

		let relay_vrf_story = RelayVRFStory([42u8; 32]);
		let assignments = compute_assignments(
			&keystore,
			relay_vrf_story.clone(),
			&config,
			(0..n_cores)
				.map(|i| {
					(
						CandidateHash(Hash::repeat_byte(i as u8)),
						CoreIndex(i as u32),
						group_for_core(i),
					)
				})
				.collect::<Vec<_>>(),
		);

		let mut counted = 0;
		for (core, assignment) in assignments {
			let mut mutated = MutatedAssignment {
				core,
				group: group_for_core(core.0 as _),
				cert: assignment.cert,
				own_group: GroupIndex(0),
				val_index: ValidatorIndex(0),
				config: config.clone(),
			};

			let expected = match f(&mut mutated) {
				None => continue,
				Some(e) => e,
			};

			counted += 1;

			let is_good = check_assignment_cert(
				mutated.core,
				mutated.val_index,
				&mutated.config,
				relay_vrf_story.clone(),
				&mutated.cert,
				mutated.group,
			)
			.is_ok();

			assert_eq!(expected, is_good)
		}

		assert!(counted > 0);
	}

	#[test]
	fn computed_assignments_pass_checks() {
		check_mutated_assignments(200, 100, 25, |_| Some(true));
	}

	#[test]
	fn check_rejects_claimed_core_out_of_bounds() {
		check_mutated_assignments(200, 100, 25, |m| {
			m.core.0 += 100;
			Some(false)
		});
	}

	#[test]
	fn check_rejects_in_backing_group() {
		check_mutated_assignments(200, 100, 25, |m| {
			m.group = m.own_group;
			Some(false)
		});
	}

	#[test]
	fn check_rejects_nonexistent_key() {
		check_mutated_assignments(200, 100, 25, |m| {
			m.val_index.0 += 200;
			Some(false)
		});
	}

	#[test]
	fn check_rejects_delay_bad_vrf() {
		check_mutated_assignments(40, 10, 8, |m| {
			match m.cert.kind.clone() {
				AssignmentCertKind::RelayVRFDelay { .. } => {
					m.cert.vrf = garbage_vrf();
					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}

	#[test]
	fn check_rejects_modulo_bad_vrf() {
		check_mutated_assignments(200, 100, 25, |m| {
			match m.cert.kind.clone() {
				AssignmentCertKind::RelayVRFModulo { .. } => {
					m.cert.vrf = garbage_vrf();
					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}

	#[test]
	fn check_rejects_modulo_sample_out_of_bounds() {
		check_mutated_assignments(200, 100, 25, |m| {
			match m.cert.kind.clone() {
				AssignmentCertKind::RelayVRFModulo { sample } => {
					m.config.relay_vrf_modulo_samples = sample;
					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}

	#[test]
	fn check_rejects_delay_claimed_core_wrong() {
		check_mutated_assignments(200, 100, 25, |m| {
			match m.cert.kind.clone() {
				AssignmentCertKind::RelayVRFDelay { .. } => {
					m.core = CoreIndex((m.core.0 + 1) % 100);
					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}

	#[test]
	fn check_rejects_modulo_core_wrong() {
		check_mutated_assignments(200, 100, 25, |m| {
			match m.cert.kind.clone() {
				AssignmentCertKind::RelayVRFModulo { .. } => {
					m.core = CoreIndex((m.core.0 + 1) % 100);
					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}
}
