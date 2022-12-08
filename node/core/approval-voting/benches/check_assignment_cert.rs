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
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use merlin::Transcript;
use parity_scale_codec::{Decode, Encode};
use polkadot_primitives::v2::Hash;
use schnorrkel::{
	vrf::{VRFOutput, VRFProof},
	Keypair,
};
use std::time::Duration;

use polkadot_node_primitives::{
	approval::{
		AssignmentCert, AssignmentCertKind, DelayTranche, RelayVRFStory, RELAY_VRF_MODULO_CONTEXT,
	},
	AvailableData, BlockData, PoV,
};
use polkadot_node_subsystem::{
	messages::{
		AllMessages, ApprovalVotingMessage, AssignmentCheckResult, AvailabilityRecoveryMessage,
	},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_overseer::HeadSupportsParachains;
use polkadot_primitives::v2::{
	CandidateCommitments, CandidateEvent, CoreIndex, GroupIndex, Header, Id as ParaId, IndexedVec,
	ValidationCode, ValidatorSignature,
};

use assert_matches::assert_matches;
use async_trait::async_trait;
use parking_lot::Mutex;
use sp_keyring::sr25519::Keyring as Sr25519Keyring;
use sp_keystore::CryptoStore;
use std::{
	pin::Pin,
	sync::{
		atomic::{AtomicBool, Ordering},
		Arc,
	},
};

// use crate::{
// 	approval_db::v1::StoredBlockRange,
// 	backend::BackendWriteOp,
// 	import::tests::{
// 		garbage_vrf, AllowedSlots, BabeEpoch, BabeEpochConfiguration, CompatibleDigestItem, Digest,
// 		DigestItem, PreDigest, SecondaryVRFPreDigest,
// 	},
// };

use ::test_helpers::{dummy_candidate_receipt, dummy_candidate_receipt_bad_sig};

fn vrf_sign(keypair: &Keypair, transcript: Transcript, extra: Transcript) -> (VRFOutput, VRFProof) {
	let ctx = schnorrkel::signing_context(RELAY_VRF_MODULO_CONTEXT);

	let (vrf_in_out, vrf_proof, _) = keypair.vrf_sign_extra(transcript, extra);

	(vrf_in_out.to_output(), vrf_proof)
}

// fn relay_vrf_modulo_transcript(relay_vrf_story: RelayVRFStory, sample: u32) -> Transcript {
// 	// combine the relay VRF story with a sample number.
// 	let mut t = Transcript::new(approval_types::RELAY_VRF_MODULO_CONTEXT);
// 	t.append_message(b"RC-VRF", &relay_vrf_story.0);
// 	sample.using_encoded(|s| t.append_message(b"sample", s));

// 	t
// }

fn check_assignment(c: &mut Criterion) {
	let mut group = c.benchmark_group("check");
	let input = 0;
	let mut transcript = Transcript::new(b"dummy");
	transcript.append_message(b"label", b"message");

	let mut extra = Transcript::new(b"extra");
	extra.append_message(b"label", b"message");

	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);
	let (output, proof) = vrf_sign(&keypair, transcript.clone(), extra.clone());

	group.bench_with_input(BenchmarkId::from_parameter(0), &input, |b, &n| {
		b.iter(|| {
			let public = &keypair.public;
			let mut transcript = Transcript::new(b"dummy");
			transcript.append_message(b"label", b"message");

			let mut extra = Transcript::new(b"extra");
			extra.append_message(b"label", b"message");

			let (vrf_in_out, _) =
				public.vrf_verify_extra(transcript, &output, &proof, extra).unwrap();
		});
	});
	group.finish();
}

fn criterion_config() -> Criterion {
	Criterion::default()
		.sample_size(15)
		.warm_up_time(Duration::from_millis(1000))
		.measurement_time(Duration::from_secs(5))
}

criterion_group!(
	name = check;
	config = criterion_config();
	targets = check_assignment,
);
criterion_main!(check);
