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

use super::*;
use polkadot_primitives::v1::{GroupIndex, ValidatorSignature};
use polkadot_node_primitives::approval::{
	AssignmentCert, AssignmentCertKind, VRFOutput, VRFProof,
	RELAY_VRF_MODULO_CONTEXT, DelayTranche,
};

use parking_lot::Mutex;
use bitvec::order::Lsb0 as BitOrderLsb0;
use std::pin::Pin;
use std::sync::Arc;
use sp_keyring::sr25519::Keyring as Sr25519Keyring;
use assert_matches::assert_matches;

const SLOT_DURATION_MILLIS: u64 = 5000;

fn slot_to_tick(t: impl Into<Slot>) -> crate::time::Tick {
	crate::time::slot_number_to_tick(SLOT_DURATION_MILLIS, t.into())
}

#[derive(Default)]
struct MockClock {
	inner: Arc<Mutex<MockClockInner>>,
}

impl MockClock {
	fn new(tick: Tick) -> Self {
		let me = Self::default();
		me.inner.lock().set_tick(tick);
		me
	}
}

impl Clock for MockClock {
	fn tick_now(&self) -> Tick {
		self.inner.lock().tick
	}

	fn wait(&self, tick: Tick) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
		let rx = self.inner.lock().register_wakeup(tick, true);

		Box::pin(async move {
			rx.await.expect("i exist in a timeless void. yet, i remain");
		})
	}
}

// This mock clock allows us to manipulate the time and
// be notified when wakeups have been triggered.
#[derive(Default)]
struct MockClockInner {
	tick: Tick,
	wakeups: Vec<(Tick, oneshot::Sender<()>)>,
}

impl MockClockInner {
	fn set_tick(&mut self, tick: Tick) {
		self.tick = tick;
		self.wakeup_all(tick);
	}

	fn wakeup_all(&mut self, up_to: Tick) {
		// This finds the position of the first wakeup after
		// the given tick, or the end of the map.
		let drain_up_to = self.wakeups.binary_search_by_key(
			&(up_to + 1),
			|w| w.0,
		).unwrap_or_else(|i| i);

		for (_, wakeup) in self.wakeups.drain(..drain_up_to) {
			let _ = wakeup.send(());
		}
	}

	// If `pre_emptive` is true, we compare the given tick to the internal
	// tick of the clock for an early return.
	//
	// Otherwise, the wakeup will only trigger alongside another wakeup of
	// equal or greater tick.
	//
	// When the pre-emptive wakeup is disabled, this can be used in combination with
	// a preceding call to `set_tick` to wait until some other wakeup at that same tick
	//  has been triggered.
	fn register_wakeup(&mut self, tick: Tick, pre_emptive: bool) -> oneshot::Receiver<()> {
		let (tx, rx) = oneshot::channel();

		let pos = self.wakeups.binary_search_by_key(
			&tick,
			|w| w.0,
		).unwrap_or_else(|i| i);

		self.wakeups.insert(pos, (tick, tx));

		if pre_emptive {
			// if `tick > self.tick`, this won't wake up the new
			// listener.
			self.wakeup_all(self.tick);
		}

		rx
	}
}

struct MockAssignmentCriteria<F>(F);

impl<F> AssignmentCriteria for MockAssignmentCriteria<F>
where
	F: Fn() -> Result<DelayTranche, criteria::InvalidAssignment>
{
	fn compute_assignments(
		&self,
		_keystore: &LocalKeystore,
		_relay_vrf_story: polkadot_node_primitives::approval::RelayVRFStory,
		_config: &criteria::Config,
		_leaving_cores: Vec<(polkadot_primitives::v1::CoreIndex, polkadot_primitives::v1::GroupIndex)>,
	) -> HashMap<polkadot_primitives::v1::CoreIndex, criteria::OurAssignment> {
		HashMap::new()
	}

	fn check_assignment_cert(
		&self,
		_claimed_core_index: polkadot_primitives::v1::CoreIndex,
		_validator_index: ValidatorIndex,
		_config: &criteria::Config,
		_relay_vrf_story: polkadot_node_primitives::approval::RelayVRFStory,
		_assignment: &polkadot_node_primitives::approval::AssignmentCert,
		_backing_group: polkadot_primitives::v1::GroupIndex,
	) -> Result<polkadot_node_primitives::approval::DelayTranche, criteria::InvalidAssignment> {
		self.0()
	}
}

#[derive(Default)]
struct TestStore {
	block_entries: HashMap<Hash, BlockEntry>,
	candidate_entries: HashMap<CandidateHash, CandidateEntry>,
}

impl DBReader for TestStore {
	fn load_block_entry(
		&self,
		block_hash: &Hash,
	) -> SubsystemResult<Option<BlockEntry>> {
		Ok(self.block_entries.get(block_hash).cloned())
	}

	fn load_candidate_entry(
		&self,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateEntry>> {
		Ok(self.candidate_entries.get(candidate_hash).cloned())
	}
}

fn blank_state() -> State<TestStore> {
	State {
		session_window: import::RollingSessionWindow::default(),
		keystore: LocalKeystore::in_memory(),
		slot_duration_millis: SLOT_DURATION_MILLIS,
		db: TestStore::default(),
		clock: Box::new(MockClock::default()),
		assignment_criteria: Box::new(MockAssignmentCriteria(|| { Ok(0) })),
	}
}

fn single_session_state(index: SessionIndex, info: SessionInfo)
	-> State<TestStore>
{
	State {
		session_window: import::RollingSessionWindow {
			earliest_session: Some(index),
			session_info: vec![info],
		},
		..blank_state()
	}
}

fn garbage_assignment_cert(kind: AssignmentCertKind) -> AssignmentCert {
	let ctx = schnorrkel::signing_context(RELAY_VRF_MODULO_CONTEXT);
	let msg = b"test-garbage";
	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);
	let (inout, proof, _) = keypair.vrf_sign(ctx.bytes(msg));
	let out = inout.to_output();

	AssignmentCert {
		kind,
		vrf: (VRFOutput(out), VRFProof(proof)),
	}
}

fn sign_approval(
	key: Sr25519Keyring,
	candidate_hash: CandidateHash,
	session_index: SessionIndex,
) -> ValidatorSignature {
	key.sign(&super::approval_signing_payload(ApprovalVote(candidate_hash), session_index)).into()
}

struct StateConfig {
	session_index: SessionIndex,
	slot: Slot,
	tick: Tick,
	validators: Vec<Sr25519Keyring>,
	validator_groups: Vec<Vec<ValidatorIndex>>,
	needed_approvals: u32,
	no_show_slots: u32,
}

impl Default for StateConfig {
	fn default() -> Self {
		StateConfig {
			session_index: 1,
			slot: Slot::from(0),
			tick: 0,
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob],
			validator_groups: vec![vec![0], vec![1]],
			needed_approvals: 1,
			no_show_slots: 2,
		}
	}
}

// one block with one candidate. Alice and Bob are in the assignment keys.
fn some_state(config: StateConfig) -> State<TestStore> {
	let StateConfig {
		session_index,
		slot,
		tick,
		validators,
		validator_groups,
		needed_approvals,
		no_show_slots,
	} = config;

	let n_validators = validators.len();

	let mut state = State {
		clock: Box::new(MockClock::new(tick)),
		..single_session_state(session_index, SessionInfo {
			validators: validators.iter().map(|v| v.public().into()).collect(),
			discovery_keys: validators.iter().map(|v| v.public().into()).collect(),
			assignment_keys: validators.iter().map(|v| v.public().into()).collect(),
			validator_groups: validator_groups.clone(),
			n_cores: validator_groups.len() as _,
			zeroth_delay_tranche_width: 5,
			relay_vrf_modulo_samples: 3,
			n_delay_tranches: 50,
			no_show_slots,
			needed_approvals,
			..Default::default()
		})
	};
	let core_index = 0.into();

	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let block_entry = approval_db::v1::BlockEntry {
		block_hash,
		session: session_index,
		slot,
		candidates: vec![(core_index, candidate_hash)],
		relay_vrf_story: Default::default(),
		approved_bitfield: bitvec::bitvec![BitOrderLsb0, u8; 0; 1],
		children: Default::default(),
	};
	let approval_entry = approval_db::v1::ApprovalEntry {
		tranches: Vec::new(),
		backing_group: GroupIndex(0),
		our_assignment: None,
		assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
		approved: false,
	};
	let candidate_entry = approval_db::v1::CandidateEntry {
		session: session_index,
		block_assignments: maplit::btreemap! {
			block_hash => approval_entry,
		},
		candidate: CandidateReceipt::default(),
		approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
	};

	state.db.block_entries.insert(block_hash, block_entry.into());
	state.db.candidate_entries.insert(candidate_hash, candidate_entry.into());
	state
}

struct TestHarness {
	clock: Arc<Mutex<MockClockInner>>,
	store: TestStore,
}

#[test]
fn rejects_bad_assignment() {
	let block_hash = Hash::repeat_byte(0x01);
	let assignment_good = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};
	let tick = 10;
	let mut state = some_state(Default::default());
	let candidate_index = 0;

	let res = check_and_import_assignment(
		&mut state,
		assignment_good.clone(),
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::Accepted);
	// Check that the assignment's been imported.
	assert!(res.1.iter().any(|action| matches!(action, Action::WriteCandidateEntry(..))));

	// unknown hash
	let assignment = IndirectAssignmentCert {
		block_hash: Hash::repeat_byte(0x02),
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let res = check_and_import_assignment(
		&mut state,
		assignment,
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::Bad);

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Err(criteria::InvalidAssignment)
		})),
		..some_state(Default::default())
	};

	// same assignment, but this time rejected
	let res = check_and_import_assignment(
		&mut state,
		assignment_good,
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::Bad);
}

#[test]
fn rejects_assignment_in_future() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_index = 0;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let tick = 9;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(10)
		})),
		..some_state(StateConfig { tick, ..Default::default() })
	};

	let res = check_and_import_assignment(
		&mut state,
		assignment.clone(),
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::TooFarInFuture);

	let tick = 10;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(10)
		})),
		..some_state(StateConfig { tick, ..Default::default() })
	};

	let res = check_and_import_assignment(
		&mut state,
		assignment.clone(),
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::TooFarInFuture);

	let tick = 11;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(10)
		})),
		..some_state(StateConfig { slot: Slot::from(6), tick, ..Default::default() })
	};

	let res = check_and_import_assignment(
		&mut state,
		assignment,
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::TooFarInFuture);
}

#[test]
fn rejects_assignment_with_unknown_candidate() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_index = 1;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let mut state = some_state(Default::default());

	let res = check_and_import_assignment(
		&mut state,
		assignment.clone(),
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::Bad);
}

#[test]
fn assignment_import_updates_candidate_entry_and_schedules_wakeup() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let candidate_index = 0;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let (res, actions) = check_and_import_assignment(
		&mut state,
		assignment.clone(),
		candidate_index,
	).unwrap();

	assert_eq!(res, AssignmentCheckResult::Accepted);
	assert_eq!(actions.len(), 2);

	assert_matches!(
		actions.get(0).unwrap(),
		Action::ScheduleWakeup {
			block_hash: b,
			candidate_hash: c,
			tick,
		} => {
			assert_eq!(b, &block_hash);
			assert_eq!(c, &candidate_hash);
			assert_eq!(tick, &slot_to_tick(0 + 2)); // current tick + no-show-duration.
		}
	);

	assert_matches!(
		actions.get(1).unwrap(),
		Action::WriteCandidateEntry(c, e) => {
			assert_eq!(c, &candidate_hash);
			assert!(e.approval_entry(&block_hash).unwrap().is_assigned(0));
		}
	);
}

#[test]
fn rejects_approval_before_assignment() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let candidate_index = 0;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index: 0,
		validator: 0,
		signature: sign_approval(Sr25519Keyring::Alice, candidate_hash, 1),
	};

	let (actions, res) = check_and_import_approval(
		&state,
		vote,
		|r| r
	).unwrap();

	assert_eq!(res, ApprovalCheckResult::Bad);
	assert!(actions.is_empty());
}

#[test]
fn rejects_approval_if_no_candidate_entry() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let candidate_index = 0;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index: 0,
		validator: 0,
		signature: sign_approval(Sr25519Keyring::Alice, candidate_hash, 1),
	};

	state.db.candidate_entries.remove(&candidate_hash);

	let (actions, res) = check_and_import_approval(
		&state,
		vote,
		|r| r
	).unwrap();

	assert_eq!(res, ApprovalCheckResult::Bad);
	assert!(actions.is_empty());
}

#[test]
fn rejects_approval_if_no_block_entry() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index = 0;

	let candidate_index = 0;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index: 0,
		validator: 0,
		signature: sign_approval(Sr25519Keyring::Alice, candidate_hash, 1),
	};

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index, 0);

	state.db.block_entries.remove(&block_hash);

	let (actions, res) = check_and_import_approval(
		&state,
		vote,
		|r| r
	).unwrap();

	assert_eq!(res, ApprovalCheckResult::Bad);
	assert!(actions.is_empty());
}

#[test]
fn accepts_and_imports_approval_after_assignment() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index = 0;

	let candidate_index = 0;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![0, 1], vec![2]],
			needed_approvals: 2,
			..Default::default()
		})
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index,
		validator: validator_index,
		signature: sign_approval(Sr25519Keyring::Alice, candidate_hash, 1),
	};

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index, 0);

	let (actions, res) = check_and_import_approval(
		&state,
		vote,
		|r| r
	).unwrap();

	assert_eq!(res, ApprovalCheckResult::Accepted);

	assert_eq!(actions.len(), 1);
	assert_matches!(
		actions.get(0).unwrap(),
		Action::WriteCandidateEntry(c_hash, c_entry) => {
			assert_eq!(c_hash, &candidate_hash);
			assert!(c_entry.approvals().get(validator_index as usize).unwrap());
			assert!(!c_entry.approval_entry(&block_hash).unwrap().is_approved());
		}
	);
}

#[test]
fn second_approval_import_is_no_op() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index = 0;

	let candidate_index = 0;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![0, 1], vec![2]],
			needed_approvals: 2,
			..Default::default()
		})
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index,
		validator: validator_index,
		signature: sign_approval(Sr25519Keyring::Alice, candidate_hash, 1),
	};

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index, 0);

	assert!(!state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.mark_approval(validator_index));

	let (actions, res) = check_and_import_approval(
		&state,
		vote,
		|r| r
	).unwrap();

	assert_eq!(res, ApprovalCheckResult::Accepted);
	assert!(actions.is_empty())
}

#[test]
fn check_and_apply_full_approval_sets_flag_and_bit() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index_a = 0;
	let validator_index_b = 1;

	let candidate_index = 0;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![0, 1], vec![2]],
			needed_approvals: 2,
			..Default::default()
		})
	};

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_a, 0);

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_b, 0);

	assert!(!state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.mark_approval(validator_index_a));

	assert!(!state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.mark_approval(validator_index_b));

	let actions = check_and_apply_full_approval(
		&state,
		None,
		candidate_hash,
		state.db.candidate_entries.get(&candidate_hash).unwrap().clone(),
		|b_hash, _a| b_hash == &block_hash,
	).unwrap();

	assert_eq!(actions.len(), 2);
	assert_matches!(
		actions.get(0).unwrap(),
		Action::WriteBlockEntry(b_entry) => {
			assert_eq!(b_entry.block_hash(), block_hash);
			assert!(b_entry.is_fully_approved());
			assert!(b_entry.is_candidate_approved(&candidate_hash));
		}
	);
	assert_matches!(
		actions.get(1).unwrap(),
		Action::WriteCandidateEntry(c_hash, c_entry) => {
			assert_eq!(c_hash, &candidate_hash);
			assert!(c_entry.approval_entry(&block_hash).unwrap().is_approved());
		}
	);
}

#[test]
fn check_and_apply_full_approval_does_not_load_cached_block_from_db() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index_a = 0;
	let validator_index_b = 1;

	let candidate_index = 0;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![0, 1], vec![2]],
			needed_approvals: 2,
			..Default::default()
		})
	};

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_a, 0);

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_b, 0);

	assert!(!state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.mark_approval(validator_index_a));

	assert!(!state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.mark_approval(validator_index_b));

	let block_entry = state.db.block_entries.remove(&block_hash).unwrap();

	let actions = check_and_apply_full_approval(
		&state,
		Some((block_hash, block_entry)),
		candidate_hash,
		state.db.candidate_entries.get(&candidate_hash).unwrap().clone(),
		|b_hash, _a| b_hash == &block_hash,
	).unwrap();

	assert_eq!(actions.len(), 2);
	assert_matches!(
		actions.get(0).unwrap(),
		Action::WriteBlockEntry(b_entry) => {
			assert_eq!(b_entry.block_hash(), block_hash);
			assert!(b_entry.is_fully_approved());
			assert!(b_entry.is_candidate_approved(&candidate_hash));
		}
	);
	assert_matches!(
		actions.get(1).unwrap(),
		Action::WriteCandidateEntry(c_hash, c_entry) => {
			assert_eq!(c_hash, &candidate_hash);
			assert!(c_entry.approval_entry(&block_hash).unwrap().is_approved());
		}
	);
}

#[test]
fn assignment_triggered_only_when_needed() {

}

#[test]
fn background_requests_are_forwarded() {

}

#[test]
fn triggered_assignment_leads_to_recovery_and_validation() {

}

#[test]
fn finalization_event_prunes() {

}

#[test]
fn load_initial_session_window() {

}

#[test]
fn adjust_session_window() {

}

#[test]
fn same_candidate_in_multiple_blocks() {

}

#[test]
fn approved_ancestor() {

}
