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

use polkadot_node_primitives::approval::{
	self as approval_types, AssignmentCert, AssignmentCertKind, DelayTranche, RelayVRFStory,
};
use polkadot_primitives::v1::{CoreIndex, ValidatorIndex, SessionInfo, AssignmentPair};
use sc_keystore::LocalKeystore;
use parity_scale_codec::{Encode, Decode};
use sp_application_crypto::Public;

use merlin::Transcript;
use schnorrkel::vrf::VRFInOut;

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::LOG_TARGET;

/// Details pertaining to our assignment on a block.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub(crate) struct OurAssignment {
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

fn relay_vrf_modulo_transcript(
	relay_vrf_story: RelayVRFStory,
	sample: u32,
) -> Transcript {
	// combine the relay VRF story with a sample number.
	let mut t = Transcript::new(approval_types::RELAY_VRF_MODULO_CONTEXT);
	t.append_message(b"RC-VRF", &relay_vrf_story.0);
	sample.using_encoded(|s| t.append_message(b"sample", s));

	t
}

fn relay_vrf_modulo_core(
	vrf_in_out: &VRFInOut,
	n_cores: u32,
) -> CoreIndex {
	let bytes: [u8; 4] = vrf_in_out.make_bytes(approval_types::CORE_RANDOMNESS_CONTEXT);

	// interpret as little-endian u32.
	let random_core = u32::from_le_bytes(bytes) % n_cores;
	CoreIndex(random_core)
}

fn relay_vrf_delay_transcript(
	relay_vrf_story: RelayVRFStory,
	core_index: CoreIndex,
) -> Transcript {
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
	let wide_tranche = u32::from_le_bytes(bytes) % (num_delay_tranches + zeroth_delay_tranche_width);

	// Consolidate early results to tranche zero so tranche zero is extra wide.
	wide_tranche.saturating_sub(zeroth_delay_tranche_width)
}

fn assigned_core_transcript(core_index: CoreIndex) -> Transcript {
	let mut t = Transcript::new(approval_types::ASSIGNED_CORE_CONTEXT);
	core_index.0.using_encoded(|s| t.append_message(b"core", s));
	t
}

/// Compute the assignments for a given block. Returns a map containing all assignments to cores in
/// the block. If more than one assignment targets the given core, only the earliest assignment is kept.
///
/// The current description of the protocol assigns every validator to check every core. But at different times.
/// The idea is that most assignments are never triggered and fall by the wayside.
pub(crate) fn compute_assignments(
	keystore: &LocalKeystore,
	relay_vrf_story: RelayVRFStory,
	session_info: &SessionInfo,
	leaving_cores: impl IntoIterator<Item = CoreIndex> + Clone,
) -> HashMap<CoreIndex, OurAssignment> {
	let (index, assignments_key): (ValidatorIndex, AssignmentPair) = {
		let key = session_info.assignment_keys.iter().enumerate()
			.filter_map(|(i, p)| match keystore.key_pair(p) {
				Ok(pair) => Some((i as ValidatorIndex, pair)),
				Err(sc_keystore::Error::PairNotFound(_)) => None,
				Err(e) => {
					tracing::warn!(target: LOG_TARGET, "Encountered keystore error: {:?}", e);
					None
				}
			})
			.next();

		match key {
			None => return Default::default(),
			Some(k) => k,
		}
	};

	let assignments_key: &sp_application_crypto::sr25519::Pair = assignments_key.as_ref();
	let assignments_key: &schnorrkel::Keypair = assignments_key.as_ref();

	let mut assignments = HashMap::new();

	// First run `RelayVRFModulo` for each sample.
	compute_relay_vrf_modulo_assignments(
		&assignments_key,
		index,
		session_info,
		relay_vrf_story.clone(),
		leaving_cores.clone(),
		&mut assignments,
	);

	// Then run `RelayVRFDelay` once for the whole block.
	compute_relay_vrf_delay_assignments(
		&assignments_key,
		index,
		session_info,
		relay_vrf_story,
		leaving_cores,
		&mut assignments,
	);

	assignments
}

fn compute_relay_vrf_modulo_assignments(
	assignments_key: &schnorrkel::Keypair,
	validator_index: ValidatorIndex,
	session_info: &SessionInfo,
	relay_vrf_story: RelayVRFStory,
	leaving_cores: impl IntoIterator<Item = CoreIndex> + Clone,
	assignments: &mut HashMap<CoreIndex, OurAssignment>,
) {
	for rvm_sample in 0..session_info.relay_vrf_modulo_samples {
		let mut core = Default::default();
		let maybe_assignment = assignments_key.vrf_sign_extra_after_check(
			relay_vrf_modulo_transcript(relay_vrf_story.clone(), rvm_sample),
			|vrf_in_out| {
				core = relay_vrf_modulo_core(&vrf_in_out, session_info.n_cores);
				if leaving_cores.clone().into_iter().any(|c| c == core) {
					Some(assigned_core_transcript(core))
				} else {
					None
				}
			}
		);

		if let Some((vrf_in_out, vrf_proof, _)) = maybe_assignment {
			// Sanity: `core` is always initialized to non-default here, as the closure above
			// has been executed.

			let cert = AssignmentCert {
				kind: AssignmentCertKind::RelayVRFModulo { sample: rvm_sample },
				vrf: (approval_types::VRFOutput(vrf_in_out.to_output()), approval_types::VRFProof(vrf_proof)),
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
	session_info: &SessionInfo,
	relay_vrf_story: RelayVRFStory,
	leaving_cores: impl IntoIterator<Item = CoreIndex>,
	assignments: &mut HashMap<CoreIndex, OurAssignment>,
) {
	for core in leaving_cores {
		let (vrf_in_out, vrf_proof, _) = assignments_key.vrf_sign(
			relay_vrf_delay_transcript(relay_vrf_story.clone(), core),
		);

		let tranche = relay_vrf_delay_tranche(
			&vrf_in_out,
			session_info.n_delay_tranches,
			session_info.zeroth_delay_tranche_width,
		);

		let cert = AssignmentCert {
			kind: AssignmentCertKind::RelayVRFDelay { core_index: core },
			vrf: (approval_types::VRFOutput(vrf_in_out.to_output()), approval_types::VRFProof(vrf_proof)),
		};

		let our_assignment = OurAssignment {
			cert,
			tranche,
			validator_index,
			triggered: false,
		};

		match assignments.entry(core) {
			Entry::Vacant(e) => { let _ = e.insert(our_assignment); }
			Entry::Occupied(mut e) => if e.get().tranche > our_assignment.tranche {
				e.insert(our_assignment);
			},
		}
	}
}

/// Assignment invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAssignment;

impl std::fmt::Display for InvalidAssignment {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "Invalid Assignment")
	}
}

impl std::error::Error for InvalidAssignment { }

/// Checks the crypto of an assignment cert. Failure conditions:
///   * Validator index out of bounds
///   * VRF signature check fails
///   * VRF output doesn't match assigned core
///   * Core is not covered by extra data in signature
///   * Core index out of bounds
///   * Sample is out of bounds
///
/// This function does not check whether the core is actually a valid assignment or not. That should be done
/// outside of the scope of this function.
pub(crate) fn check_assignment_cert(
	claimed_core_index: CoreIndex,
	validator_index: ValidatorIndex,
	session_info: &SessionInfo,
	relay_vrf_story: RelayVRFStory,
	assignment: &AssignmentCert,
) -> Result<DelayTranche, InvalidAssignment> {
	let validator_public = session_info.assignment_keys
		.get(validator_index as usize)
		.ok_or(InvalidAssignment)?;

	let public = schnorrkel::PublicKey::from_bytes(validator_public.as_slice())
		.map_err(|_| InvalidAssignment)?;

	if claimed_core_index.0 >= session_info.n_cores {
		return Err(InvalidAssignment);
	}

	let &(ref vrf_output, ref vrf_proof) = &assignment.vrf;
	match assignment.kind {
		AssignmentCertKind::RelayVRFModulo { sample } => {
			if sample >= session_info.relay_vrf_modulo_samples {
				return Err(InvalidAssignment);
			}

			let (vrf_in_out, _) = public.vrf_verify_extra(
				relay_vrf_modulo_transcript(relay_vrf_story, sample),
				&vrf_output.0,
				&vrf_proof.0,
				assigned_core_transcript(claimed_core_index)
			).map_err(|_| InvalidAssignment)?;

			// ensure that the `vrf_in_out` actually gives us the claimed core.
			if relay_vrf_modulo_core(&vrf_in_out, session_info.n_cores) == claimed_core_index {
				Ok(0)
			} else {
				Err(InvalidAssignment)
			}
		}
		AssignmentCertKind::RelayVRFDelay { core_index } => {
			if core_index != claimed_core_index {
				return Err(InvalidAssignment);
			}

			let (vrf_in_out, _) = public.vrf_verify(
				relay_vrf_delay_transcript(relay_vrf_story, core_index),
				&vrf_output.0,
				&vrf_proof.0,
			).map_err(|_| InvalidAssignment)?;

			Ok(relay_vrf_delay_tranche(
				&vrf_in_out,
				session_info.n_delay_tranches,
				session_info.zeroth_delay_tranche_width,
			))
		}
	}
}
