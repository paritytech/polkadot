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

use polkadot_node_primitives::approval::{self as approval_types, AssignmentCert, DelayTranche, RelayVRFStory};
use polkadot_primitives::v1::{CoreIndex, ValidatorIndex, SessionInfo, AssignmentPair};
use sc_keystore::LocalKeystore;
use parity_scale_codec::{Encode, Decode};
use merlin::Transcript;
use byteorder::{ByteOrder, LE};
use schnorrkel::vrf::VRFInOut;

use std::collections::HashMap;

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
	let random_core = LE::read_u32(&bytes) % n_cores;
	CoreIndex(random_core)
}

fn assigned_core_transcript(core_index: CoreIndex) -> Transcript {
	let mut t = Transcript::new(approval_types::ASSIGNED_CORE_CONTEXT);
	core_index.0.using_encoded(|s| t.append_message(b"core", s));
	t
}

/// Compute the assignments for a given block. Returns a map containing all assignments to cores in
/// the block. If more than one assignment targets the given core, only the earliest assignment is kept.
pub(crate) fn compute_assignment(
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
				kind: approval_types::AssignmentCertKind::RelayVRFModulo { sample: rvm_sample },
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
