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

//! Assignment criteria VRF generation and checking.

use parity_scale_codec::{Decode, Encode};
use polkadot_node_primitives::approval::{
	self as approval_types,
	v1::{AssignmentCert, AssignmentCertKind, DelayTranche, RelayVRFStory},
	v2::{AssignmentCertKindV2, AssignmentCertV2, CoreBitfield, VrfOutput, VrfProof, VrfSignature},
};
use polkadot_primitives::{
	AssignmentId, AssignmentPair, CandidateHash, CoreIndex, GroupIndex, IndexedVec, SessionInfo,
	ValidatorIndex,
};
use sc_keystore::LocalKeystore;
use sp_application_crypto::ByteArray;

use merlin::Transcript;
use schnorrkel::vrf::VRFInOut;

use itertools::Itertools;
use std::collections::{hash_map::Entry, HashMap};

use super::LOG_TARGET;

/// Details pertaining to our assignment on a block.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub struct OurAssignment {
	cert: AssignmentCertV2,
	tranche: DelayTranche,
	validator_index: ValidatorIndex,
	// Whether the assignment has been triggered already.
	triggered: bool,
}

impl OurAssignment {
	pub(crate) fn cert(&self) -> &AssignmentCertV2 {
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

impl From<crate::approval_db::v2::OurAssignment> for OurAssignment {
	fn from(entry: crate::approval_db::v2::OurAssignment) -> Self {
		OurAssignment {
			cert: entry.cert,
			tranche: entry.tranche,
			validator_index: entry.validator_index,
			triggered: entry.triggered,
		}
	}
}

impl From<OurAssignment> for crate::approval_db::v2::OurAssignment {
	fn from(entry: OurAssignment) -> Self {
		Self {
			cert: entry.cert,
			tranche: entry.tranche,
			validator_index: entry.validator_index,
			triggered: entry.triggered,
		}
	}
}

// Combines the relay VRF story with a sample number if any.
fn relay_vrf_modulo_transcript_inner(
	mut transcript: Transcript,
	relay_vrf_story: RelayVRFStory,
	sample: Option<u32>,
) -> Transcript {
	transcript.append_message(b"RC-VRF", &relay_vrf_story.0);

	if let Some(sample) = sample {
		sample.using_encoded(|s| transcript.append_message(b"sample", s));
	}

	transcript
}

fn relay_vrf_modulo_transcript_v1(relay_vrf_story: RelayVRFStory, sample: u32) -> Transcript {
	relay_vrf_modulo_transcript_inner(
		Transcript::new(approval_types::v1::RELAY_VRF_MODULO_CONTEXT),
		relay_vrf_story,
		Some(sample),
	)
}

fn relay_vrf_modulo_transcript_v2(relay_vrf_story: RelayVRFStory) -> Transcript {
	relay_vrf_modulo_transcript_inner(
		Transcript::new(approval_types::v2::RELAY_VRF_MODULO_CONTEXT),
		relay_vrf_story,
		None,
	)
}

/// A hard upper bound on num_cores * target_checkers / num_validators
const MAX_MODULO_SAMPLES: usize = 40;

use std::convert::AsMut;

fn clone_into_array<A, T>(slice: &[T]) -> A
where
	A: Default + AsMut<[T]>,
	T: Clone,
{
	let mut a = A::default();
	<A as AsMut<[T]>>::as_mut(&mut a).clone_from_slice(slice);
	a
}

struct BigArray(pub [u8; MAX_MODULO_SAMPLES * 4]);

impl Default for BigArray {
	fn default() -> Self {
		BigArray([0u8; MAX_MODULO_SAMPLES * 4])
	}
}

impl AsMut<[u8]> for BigArray {
	fn as_mut(&mut self) -> &mut [u8] {
		self.0.as_mut()
	}
}

/// Takes the VRF output as input and returns a Vec of cores the validator is assigned
/// to as a tranche0 checker.
fn relay_vrf_modulo_cores(
	vrf_in_out: &VRFInOut,
	// Configuration - `relay_vrf_modulo_samples`.
	num_samples: u32,
	// Configuration - `n_cores`.
	max_cores: u32,
) -> Vec<CoreIndex> {
	if num_samples as usize > MAX_MODULO_SAMPLES {
		gum::warn!(
			target: LOG_TARGET,
			n_cores = max_cores,
			num_samples,
			max_modulo_samples = MAX_MODULO_SAMPLES,
			"`num_samples` is greater than `MAX_MODULO_SAMPLES`",
		);
	}

	vrf_in_out
		.make_bytes::<BigArray>(approval_types::v2::CORE_RANDOMNESS_CONTEXT)
		.0
		.chunks_exact(4)
		.take(num_samples as usize)
		.map(move |sample| CoreIndex(u32::from_le_bytes(clone_into_array(&sample)) % max_cores))
		.unique()
		.collect::<Vec<CoreIndex>>()
}

fn relay_vrf_modulo_core(vrf_in_out: &VRFInOut, n_cores: u32) -> CoreIndex {
	let bytes: [u8; 4] = vrf_in_out.make_bytes(approval_types::v1::CORE_RANDOMNESS_CONTEXT);

	// interpret as little-endian u32.
	let random_core = u32::from_le_bytes(bytes) % n_cores;
	CoreIndex(random_core)
}

fn relay_vrf_delay_transcript(relay_vrf_story: RelayVRFStory, core_index: CoreIndex) -> Transcript {
	let mut t = Transcript::new(approval_types::v1::RELAY_VRF_DELAY_CONTEXT);
	t.append_message(b"RC-VRF", &relay_vrf_story.0);
	core_index.0.using_encoded(|s| t.append_message(b"core", s));
	t
}

fn relay_vrf_delay_tranche(
	vrf_in_out: &VRFInOut,
	num_delay_tranches: u32,
	zeroth_delay_tranche_width: u32,
) -> DelayTranche {
	let bytes: [u8; 4] = vrf_in_out.make_bytes(approval_types::v1::TRANCHE_RANDOMNESS_CONTEXT);

	// interpret as little-endian u32 and reduce by the number of tranches.
	let wide_tranche =
		u32::from_le_bytes(bytes) % (num_delay_tranches + zeroth_delay_tranche_width);

	// Consolidate early results to tranche zero so tranche zero is extra wide.
	wide_tranche.saturating_sub(zeroth_delay_tranche_width)
}

fn assigned_core_transcript(core_index: CoreIndex) -> Transcript {
	let mut t = Transcript::new(approval_types::v1::ASSIGNED_CORE_CONTEXT);
	core_index.0.using_encoded(|s| t.append_message(b"core", s));
	t
}

/// Information about the world assignments are being produced in.
#[derive(Clone, Debug)]
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
		claimed_core_bitfield: CoreBitfield,
		validator_index: ValidatorIndex,
		config: &Config,
		relay_vrf_story: RelayVRFStory,
		assignment: &AssignmentCertV2,
		// Backing groups for each "leaving core".
		backing_groups: Vec<GroupIndex>,
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
		compute_assignments(keystore, relay_vrf_story, config, leaving_cores, false)
	}

	fn check_assignment_cert(
		&self,
		claimed_core_bitfield: CoreBitfield,
		validator_index: ValidatorIndex,
		config: &Config,
		relay_vrf_story: RelayVRFStory,
		assignment: &AssignmentCertV2,
		backing_groups: Vec<GroupIndex>,
	) -> Result<DelayTranche, InvalidAssignment> {
		check_assignment_cert(
			claimed_core_bitfield,
			validator_index,
			config,
			relay_vrf_story,
			assignment,
			backing_groups,
		)
	}
}

/// Compute the assignments for a given block. Returns a map containing all assignments to cores in
/// the block. If more than one assignment targets the given core, only the earliest assignment is
/// kept.
///
/// The `leaving_cores` parameter indicates all cores within the block where a candidate was
/// included, as well as the group index backing those.
///
/// The current description of the protocol assigns every validator to check every core. But at
/// different times. The idea is that most assignments are never triggered and fall by the wayside.
///
/// This will not assign to anything the local validator was part of the backing group for.
pub(crate) fn compute_assignments(
	keystore: &LocalKeystore,
	relay_vrf_story: RelayVRFStory,
	config: &Config,
	leaving_cores: impl IntoIterator<Item = (CandidateHash, CoreIndex, GroupIndex)> + Clone,
	enable_v2_assignments: bool,
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
		.filter(|(_, _, g)| !is_in_backing_group(&config.validator_groups, index, *g))
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
	if enable_v2_assignments {
		compute_relay_vrf_modulo_assignments_v2(
			&assignments_key,
			index,
			config,
			relay_vrf_story.clone(),
			leaving_cores.clone(),
			&mut assignments,
		);
	} else {
		compute_relay_vrf_modulo_assignments_v1(
			&assignments_key,
			index,
			config,
			relay_vrf_story.clone(),
			leaving_cores.clone(),
			&mut assignments,
		);
	}

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

fn compute_relay_vrf_modulo_assignments_v1(
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
				relay_vrf_modulo_transcript_v1(relay_vrf_story.clone(), rvm_sample),
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
				vrf: VrfSignature {
					output: VrfOutput(vrf_in_out.to_output()),
					proof: VrfProof(vrf_proof),
				},
			};

			// All assignments of type RelayVRFModulo have tranche 0.
			assignments.entry(core).or_insert(OurAssignment {
				cert: cert.into(),
				tranche: 0,
				validator_index,
				triggered: false,
			});
		}
	}
}

fn assigned_cores_transcript(core_bitfield: &CoreBitfield) -> Transcript {
	let mut t = Transcript::new(approval_types::v2::ASSIGNED_CORE_CONTEXT);
	core_bitfield.using_encoded(|s| t.append_message(b"cores", s));
	t
}

fn compute_relay_vrf_modulo_assignments_v2(
	assignments_key: &schnorrkel::Keypair,
	validator_index: ValidatorIndex,
	config: &Config,
	relay_vrf_story: RelayVRFStory,
	leaving_cores: Vec<(CandidateHash, CoreIndex)>,
	assignments: &mut HashMap<CoreIndex, OurAssignment>,
) {
	let mut assigned_cores = Vec::new();
	let leaving_cores = leaving_cores.iter().map(|(_, core)| core).collect::<Vec<_>>();

	let maybe_assignment = {
		let assigned_cores = &mut assigned_cores;
		assignments_key.vrf_sign_extra_after_check(
			relay_vrf_modulo_transcript_v2(relay_vrf_story.clone()),
			|vrf_in_out| {
				*assigned_cores = relay_vrf_modulo_cores(
					&vrf_in_out,
					config.relay_vrf_modulo_samples,
					config.n_cores,
				)
				.into_iter()
				.filter(|core| leaving_cores.contains(&core))
				.collect::<Vec<CoreIndex>>();

				if !assigned_cores.is_empty() {
					gum::trace!(
						target: LOG_TARGET,
						?assigned_cores,
						?validator_index,
						tranche = 0,
						"RelayVRFModuloCompact Assignment."
					);

					let assignment_bitfield: CoreBitfield = assigned_cores
						.clone()
						.try_into()
						.expect("Just checked `!assigned_cores.is_empty()`; qed");

					Some(assigned_cores_transcript(&assignment_bitfield))
				} else {
					None
				}
			},
		)
	};

	if let Some(assignment) = maybe_assignment.map(|(vrf_in_out, vrf_proof, _)| {
		let assignment_bitfield: CoreBitfield = assigned_cores
			.clone()
			.try_into()
			.expect("Just checked `!assigned_cores.is_empty()`; qed");

		let cert = AssignmentCertV2 {
			kind: AssignmentCertKindV2::RelayVRFModuloCompact {
				core_bitfield: assignment_bitfield.clone(),
			},
			vrf: VrfSignature {
				output: VrfOutput(vrf_in_out.to_output()),
				proof: VrfProof(vrf_proof),
			},
		};

		// All assignments of type RelayVRFModulo have tranche 0.
		OurAssignment { cert, tranche: 0, validator_index, triggered: false }
	}) {
		for core_index in assigned_cores {
			assignments.insert(core_index, assignment.clone());
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

		let cert = AssignmentCertV2 {
			kind: AssignmentCertKindV2::RelayVRFDelay { core_index: core },
			vrf: VrfSignature {
				output: VrfOutput(vrf_in_out.to_output()),
				proof: VrfProof(vrf_proof),
			},
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
	InvalidArguments,
	/// Assignment vrf check resulted in 0 assigned cores.
	NullAssignment,
}

/// Checks the crypto of an assignment cert. Failure conditions:
///   * Validator index out of bounds
///   * VRF signature check fails
///   * VRF output doesn't match assigned cores
///   * Core is not covered by extra data in signature
///   * Core index out of bounds
///   * Sample is out of bounds
///   * Validator is present in backing group.
///
/// This function does not check whether the core is actually a valid assignment or not. That should
/// be done outside the scope of this function.
pub(crate) fn check_assignment_cert(
	claimed_core_indices: CoreBitfield,
	validator_index: ValidatorIndex,
	config: &Config,
	relay_vrf_story: RelayVRFStory,
	assignment: &AssignmentCertV2,
	backing_groups: Vec<GroupIndex>,
) -> Result<DelayTranche, InvalidAssignment> {
	use InvalidAssignmentReason as Reason;

	let validator_public = config
		.assignment_keys
		.get(validator_index.0 as usize)
		.ok_or(InvalidAssignment(Reason::ValidatorIndexOutOfBounds))?;

	let public = schnorrkel::PublicKey::from_bytes(validator_public.as_slice())
		.map_err(|_| InvalidAssignment(Reason::InvalidAssignmentKey))?;

	// Check that we have all backing groups for claimed cores.
	if claimed_core_indices.count_ones() == 0 ||
		claimed_core_indices.count_ones() != backing_groups.len()
	{
		return Err(InvalidAssignment(Reason::InvalidArguments))
	}

	// Check that the validator was not part of the backing group
	// and not already assigned.
	for (claimed_core, backing_group) in claimed_core_indices.iter_ones().zip(backing_groups.iter())
	{
		if claimed_core >= config.n_cores as usize {
			return Err(InvalidAssignment(Reason::CoreIndexOutOfBounds))
		}

		let is_in_backing =
			is_in_backing_group(&config.validator_groups, validator_index, *backing_group);

		if is_in_backing {
			return Err(InvalidAssignment(Reason::IsInBackingGroup))
		}
	}

	let vrf_output = &assignment.vrf.output;
	let vrf_proof = &assignment.vrf.proof;
	let first_claimed_core_index =
		claimed_core_indices.first_one().expect("Checked above; qed") as u32;

	match &assignment.kind {
		AssignmentCertKindV2::RelayVRFModuloCompact { core_bitfield } => {
			// Check that claimed core bitfield match the one from certificate.
			if &claimed_core_indices != core_bitfield {
				return Err(InvalidAssignment(Reason::VRFModuloCoreIndexMismatch))
			}

			let (vrf_in_out, _) = public
				.vrf_verify_extra(
					relay_vrf_modulo_transcript_v2(relay_vrf_story),
					&vrf_output.0,
					&vrf_proof.0,
					assigned_cores_transcript(core_bitfield),
				)
				.map_err(|_| InvalidAssignment(Reason::VRFModuloOutputMismatch))?;

			let resulting_cores = relay_vrf_modulo_cores(
				&vrf_in_out,
				config.relay_vrf_modulo_samples,
				config.n_cores,
			);

			// Currently validators can opt out of checking specific cores.
			// This is the same issue to how validator can opt out and not send their assignments in the first place.
			// Ensure that the `vrf_in_out` actually includes all of the claimed cores.
			for claimed_core_index in claimed_core_indices.iter_ones() {
				if !resulting_cores.contains(&CoreIndex(claimed_core_index as u32)) {
					gum::debug!(
						target: LOG_TARGET,
						?resulting_cores,
						?claimed_core_indices,
						vrf_modulo_cores = ?resulting_cores,
						"Assignment claimed cores mismatch",
					);
					return Err(InvalidAssignment(Reason::VRFModuloCoreIndexMismatch))
				}
			}

			Ok(0)
		},
		AssignmentCertKindV2::RelayVRFModulo { sample } => {
			if *sample >= config.relay_vrf_modulo_samples {
				return Err(InvalidAssignment(Reason::SampleOutOfBounds))
			}

			// Enforce claimed candidates is 1.
			if claimed_core_indices.count_ones() != 1 {
				gum::warn!(
					target: LOG_TARGET,
					?claimed_core_indices,
					"`RelayVRFModulo` assignment must always claim 1 core",
				);
				return Err(InvalidAssignment(Reason::InvalidArguments))
			}

			let (vrf_in_out, _) = public
				.vrf_verify_extra(
					relay_vrf_modulo_transcript_v1(relay_vrf_story, *sample),
					&vrf_output.0,
					&vrf_proof.0,
					assigned_core_transcript(CoreIndex(first_claimed_core_index)),
				)
				.map_err(|_| InvalidAssignment(Reason::VRFModuloOutputMismatch))?;

			let core = relay_vrf_modulo_core(&vrf_in_out, config.n_cores);
			// ensure that the `vrf_in_out` actually gives us the claimed core.
			if core.0 == first_claimed_core_index {
				Ok(0)
			} else {
				gum::debug!(
					target: LOG_TARGET,
					?core,
					?claimed_core_indices,
					"Assignment claimed cores mismatch",
				);
				Err(InvalidAssignment(Reason::VRFModuloCoreIndexMismatch))
			}
		},
		AssignmentCertKindV2::RelayVRFDelay { core_index } => {
			// Enforce claimed candidates is 1.
			if claimed_core_indices.count_ones() != 1 {
				gum::debug!(
					target: LOG_TARGET,
					?claimed_core_indices,
					"`RelayVRFDelay` assignment must always claim 1 core",
				);
				return Err(InvalidAssignment(Reason::InvalidArguments))
			}

			if core_index.0 != first_claimed_core_index {
				return Err(InvalidAssignment(Reason::VRFDelayCoreIndexMismatch))
			}

			let (vrf_in_out, _) = public
				.vrf_verify(
					relay_vrf_delay_transcript(relay_vrf_story, *core_index),
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

/// Migration helpers.
impl From<crate::approval_db::v1::OurAssignment> for OurAssignment {
	fn from(value: crate::approval_db::v1::OurAssignment) -> Self {
		Self {
			cert: value.cert.into(),
			tranche: value.tranche,
			validator_index: value.validator_index,
			// Whether the assignment has been triggered already.
			triggered: value.triggered,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::import::tests::garbage_vrf_signature;
	use polkadot_primitives::{Hash, ASSIGNMENT_KEY_TYPE_ID};
	use sp_application_crypto::sr25519;
	use sp_core::crypto::Pair as PairT;
	use sp_keyring::sr25519::Keyring as Sr25519Keyring;
	use sp_keystore::Keystore;

	// sets up a keystore with the given keyring accounts.
	fn make_keystore(accounts: &[Sr25519Keyring]) -> LocalKeystore {
		let store = LocalKeystore::in_memory();

		for s in accounts.iter().copied().map(|k| k.to_seed()) {
			store.sr25519_generate_new(ASSIGNMENT_KEY_TYPE_ID, Some(s.as_str())).unwrap();
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

	#[test]
	fn assignments_produced_for_non_backing() {
		let keystore = make_keystore(&[Sr25519Keyring::Alice]);

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
				relay_vrf_modulo_samples: 10,
				n_delay_tranches: 40,
			},
			vec![(c_a, CoreIndex(0), GroupIndex(1)), (c_b, CoreIndex(1), GroupIndex(0))],
			false,
		);

		// Note that alice is in group 0, which was the backing group for core 1.
		// Alice should have self-assigned to check core 0 but not 1.
		assert_eq!(assignments.len(), 1);
		assert!(assignments.get(&CoreIndex(0)).is_some());
	}

	#[test]
	fn assign_to_nonzero_core() {
		let keystore = make_keystore(&[Sr25519Keyring::Alice]);

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
				relay_vrf_modulo_samples: 10,
				n_delay_tranches: 40,
			},
			vec![(c_a, CoreIndex(0), GroupIndex(0)), (c_b, CoreIndex(1), GroupIndex(1))],
			false,
		);

		assert_eq!(assignments.len(), 1);
		assert!(assignments.get(&CoreIndex(1)).is_some());
	}

	#[test]
	fn succeeds_empty_for_0_cores() {
		let keystore = make_keystore(&[Sr25519Keyring::Alice]);

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
				relay_vrf_modulo_samples: 10,
				n_delay_tranches: 40,
			},
			vec![],
			false,
		);

		assert!(assignments.is_empty());
	}

	#[derive(Debug)]
	struct MutatedAssignment {
		cores: CoreBitfield,
		cert: AssignmentCertV2,
		groups: Vec<GroupIndex>,
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
		let keystore = make_keystore(&[Sr25519Keyring::Alice]);

		let group_for_core = |i| GroupIndex(((i + rotation_offset) % n_cores) as _);

		let config = Config {
			assignment_keys: assignment_keys_plus_random(
				&[Sr25519Keyring::Alice],
				n_validators - 1,
			),
			validator_groups: basic_groups(n_validators, n_cores),
			n_cores: n_cores as u32,
			zeroth_delay_tranche_width: 10,
			relay_vrf_modulo_samples: 15,
			n_delay_tranches: 40,
		};

		let relay_vrf_story = RelayVRFStory([42u8; 32]);
		let mut assignments = compute_assignments(
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
			false,
		);

		// Extend with v2 assignments as well
		assignments.extend(compute_assignments(
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
			true,
		));

		let mut counted = 0;
		for (core, assignment) in assignments {
			let cores = match assignment.cert.kind.clone() {
				AssignmentCertKindV2::RelayVRFModuloCompact { core_bitfield } => core_bitfield,
				AssignmentCertKindV2::RelayVRFModulo { sample: _ } => core.into(),
				AssignmentCertKindV2::RelayVRFDelay { core_index } => core_index.into(),
			};

			let mut mutated = MutatedAssignment {
				cores: cores.clone(),
				groups: cores.iter_ones().map(|core| group_for_core(core)).collect(),
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
				mutated.cores,
				mutated.val_index,
				&mutated.config,
				relay_vrf_story.clone(),
				&mutated.cert,
				mutated.groups,
			)
			.is_ok();

			assert_eq!(expected, is_good);
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
			m.cores = CoreIndex(100).into();
			Some(false)
		});
	}

	#[test]
	fn check_rejects_in_backing_group() {
		check_mutated_assignments(200, 100, 25, |m| {
			m.groups[0] = m.own_group;
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
			let vrf_signature = garbage_vrf_signature();
			match m.cert.kind.clone() {
				AssignmentCertKindV2::RelayVRFDelay { .. } => {
					m.cert.vrf = vrf_signature;
					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}

	#[test]
	fn check_rejects_modulo_bad_vrf() {
		check_mutated_assignments(200, 100, 25, |m| {
			let vrf_signature = garbage_vrf_signature();
			match m.cert.kind.clone() {
				AssignmentCertKindV2::RelayVRFModulo { .. } => {
					m.cert.vrf = vrf_signature;
					Some(false)
				},
				AssignmentCertKindV2::RelayVRFModuloCompact { .. } => {
					m.cert.vrf = vrf_signature;
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
				AssignmentCertKindV2::RelayVRFModulo { sample } => {
					m.config.relay_vrf_modulo_samples = sample;
					Some(false)
				},
				AssignmentCertKindV2::RelayVRFModuloCompact { core_bitfield: _ } => Some(true),
				_ => None, // skip everything else.
			}
		});
	}

	#[test]
	fn check_rejects_delay_claimed_core_wrong() {
		check_mutated_assignments(200, 100, 25, |m| {
			match m.cert.kind.clone() {
				AssignmentCertKindV2::RelayVRFDelay { .. } => {
					// for core in &mut m.cores {
					// 	core.0 = (core.0 + 1) % 100;
					// }
					m.cores = CoreIndex((m.cores.first_one().unwrap() + 1) as u32 % 100).into();
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
				AssignmentCertKindV2::RelayVRFModulo { .. } |
				AssignmentCertKindV2::RelayVRFModuloCompact { .. } => {
					m.cores = CoreIndex((m.cores.first_one().unwrap() + 1) as u32 % 100).into();

					Some(false)
				},
				_ => None, // skip everything else.
			}
		});
	}
}
