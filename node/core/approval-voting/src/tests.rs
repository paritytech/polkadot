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
use polkadot_primitives::v1::{CoreIndex, GroupIndex, ValidatorSignature};
use polkadot_node_primitives::approval::{
	AssignmentCert, AssignmentCertKind, VRFOutput, VRFProof,
	RELAY_VRF_MODULO_CONTEXT, DelayTranche,
};
use polkadot_node_subsystem_test_helpers::make_subsystem_context;
use polkadot_subsystem::messages::AllMessages;
use sp_core::testing::TaskExecutor;

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

#[derive(Default, Clone)]
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

struct MockAssignmentCriteria<Compute, Check>(Compute, Check);

impl<Compute, Check> AssignmentCriteria for MockAssignmentCriteria<Compute, Check>
where
	Compute: Fn() -> HashMap<polkadot_primitives::v1::CoreIndex, criteria::OurAssignment>,
	Check: Fn() -> Result<DelayTranche, criteria::InvalidAssignment>
{
	fn compute_assignments(
		&self,
		_keystore: &LocalKeystore,
		_relay_vrf_story: polkadot_node_primitives::approval::RelayVRFStory,
		_config: &criteria::Config,
		_leaving_cores: Vec<(polkadot_primitives::v1::CoreIndex, polkadot_primitives::v1::GroupIndex)>,
	) -> HashMap<polkadot_primitives::v1::CoreIndex, criteria::OurAssignment> {
		self.0()
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
		self.1()
	}
}

impl<F> MockAssignmentCriteria<
	fn() -> HashMap<polkadot_primitives::v1::CoreIndex, criteria::OurAssignment>,
	F,
> {
	fn check_only(f: F) -> Self {
		MockAssignmentCriteria(Default::default, f)
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
		keystore: Arc::new(LocalKeystore::in_memory()),
		slot_duration_millis: SLOT_DURATION_MILLIS,
		db: TestStore::default(),
		clock: Box::new(MockClock::default()),
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| { Ok(0) })),
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
			validator_groups: vec![vec![ValidatorIndex(0)], vec![ValidatorIndex(1)]],
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

	add_block(
		&mut state.db,
		block_hash,
		session_index,
		slot,
	);

	add_candidate_to_block(
		&mut state.db,
		block_hash,
		candidate_hash,
		n_validators,
		core_index,
		GroupIndex(0),
	);

	state
}

fn add_block(
	db: &mut TestStore,
	block_hash: Hash,
	session: SessionIndex,
	slot: Slot,
) {
	db.block_entries.insert(
		block_hash,
		approval_db::v1::BlockEntry {
			block_hash,
			session,
			slot,
			candidates: Vec::new(),
			relay_vrf_story: Default::default(),
			approved_bitfield: Default::default(),
			children: Default::default(),
		}.into(),
	);
}

fn add_candidate_to_block(
	db: &mut TestStore,
	block_hash: Hash,
	candidate_hash: CandidateHash,
	n_validators: usize,
	core: CoreIndex,
	backing_group: GroupIndex,
) {
	let mut block_entry = db.block_entries.get(&block_hash).unwrap().clone();

	let candidate_entry = db.candidate_entries
		.entry(candidate_hash)
		.or_insert_with(|| approval_db::v1::CandidateEntry {
			session: block_entry.session(),
			block_assignments: Default::default(),
			candidate: CandidateReceipt::default(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
		}.into());

	block_entry.add_candidate(core, candidate_hash);

	candidate_entry.add_approval_entry(
		block_hash,
		approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group,
			our_assignment: None,
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
			approved: false,
		}.into(),
	);

	db.block_entries.insert(block_hash, block_entry);
}

#[test]
fn rejects_bad_assignment() {
	let block_hash = Hash::repeat_byte(0x01);
	let assignment_good = IndirectAssignmentCert {
		block_hash,
		validator: ValidatorIndex(0),
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};
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
		validator: ValidatorIndex(0),
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
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
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
		validator: ValidatorIndex(0),
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let tick = 9;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(move || {
			Ok((tick + 20) as _)
		})),
		..some_state(StateConfig { tick, ..Default::default() })
	};

	let res = check_and_import_assignment(
		&mut state,
		assignment.clone(),
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::TooFarInFuture);

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(move || {
			Ok((tick + 20 - 1) as _)
		})),
		..some_state(StateConfig { tick, ..Default::default() })
	};

	let res = check_and_import_assignment(
		&mut state,
		assignment.clone(),
		candidate_index,
	).unwrap();
	assert_eq!(res.0, AssignmentCheckResult::Accepted);
}

#[test]
fn rejects_assignment_with_unknown_candidate() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_index = 1;
	let assignment = IndirectAssignmentCert {
		block_hash,
		validator: ValidatorIndex(0),
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
		validator: ValidatorIndex(0),
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
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
			assert!(e.approval_entry(&block_hash).unwrap().is_assigned(ValidatorIndex(0)));
		}
	);
}

#[test]
fn rejects_approval_before_assignment() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index: 0,
		validator: ValidatorIndex(0),
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

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index: 0,
		validator: ValidatorIndex(0),
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
	let validator_index = ValidatorIndex(0);

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(Default::default())
	};

	let vote = IndirectSignedApprovalVote {
		block_hash,
		candidate_index: 0,
		validator: ValidatorIndex(0),
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
	let validator_index = ValidatorIndex(0);

	let candidate_index = 0;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![ValidatorIndex(0), ValidatorIndex(1)], vec![ValidatorIndex(2)]],
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
			assert!(c_entry.approvals().get(validator_index.0 as usize).unwrap());
			assert!(!c_entry.approval_entry(&block_hash).unwrap().is_approved());
		}
	);
}

#[test]
fn second_approval_import_is_no_op() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index = ValidatorIndex(0);

	let candidate_index = 0;
	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![ValidatorIndex(0), ValidatorIndex(1)], vec![ValidatorIndex(2)]],
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
	let validator_index_a = ValidatorIndex(0);
	let validator_index_b = ValidatorIndex(1);

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![ValidatorIndex(0), ValidatorIndex(1)], vec![ValidatorIndex(2)]],
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
	let validator_index_a = ValidatorIndex(0);
	let validator_index_b = ValidatorIndex(1);

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![ValidatorIndex(0), ValidatorIndex(1)], vec![ValidatorIndex(2)]],
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
fn assignment_triggered_by_all_with_less_than_supermajority() {
	let block_hash = Hash::repeat_byte(0x01);

	let mut candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: 1,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	// 2-of-4
	candidate_entry
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, ValidatorIndex(0), 0);

	candidate_entry
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, ValidatorIndex(1), 0);

	candidate_entry.mark_approval(ValidatorIndex(0));
	candidate_entry.mark_approval(ValidatorIndex(1));

	let tranche_now = 1;
	assert!(should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::All,
		tranche_now,
	));
}

#[test]
fn assignment_not_triggered_by_all_with_supermajority() {
	let block_hash = Hash::repeat_byte(0x01);

	let mut candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: 1,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	// 3-of-4
	candidate_entry
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, ValidatorIndex(0), 0);

	candidate_entry
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, ValidatorIndex(1), 0);

	candidate_entry
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, ValidatorIndex(2), 0);

	candidate_entry.mark_approval(ValidatorIndex(0));
	candidate_entry.mark_approval(ValidatorIndex(1));
	candidate_entry.mark_approval(ValidatorIndex(2));

	let tranche_now = 1;
	assert!(!should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::All,
		tranche_now,
	));
}

#[test]
fn assignment_not_triggered_if_already_triggered() {
	let block_hash = Hash::repeat_byte(0x01);

	let candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: 1,
				validator_index: ValidatorIndex(4),
				triggered: true,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	let tranche_now = 1;
	assert!(!should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::All,
		tranche_now,
	));
}

#[test]
fn assignment_not_triggered_by_exact() {
	let block_hash = Hash::repeat_byte(0x01);

	let candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: 1,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	let tranche_now = 1;
	assert!(!should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::Exact { needed: 2, next_no_show: None, tolerated_missing: 0 },
		tranche_now,
	));
}

#[test]
fn assignment_not_triggered_more_than_maximum() {
	let block_hash = Hash::repeat_byte(0x01);
	let maximum_broadcast = 10;

	let candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: maximum_broadcast + 1,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	let tranche_now = 50;
	assert!(!should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::Pending {
			maximum_broadcast,
			clock_drift: 0,
			considered: 10,
			next_no_show: None,
		},
		tranche_now,
	));
}

#[test]
fn assignment_triggered_if_at_maximum() {
	let block_hash = Hash::repeat_byte(0x01);
	let maximum_broadcast = 10;

	let candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: maximum_broadcast,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	let tranche_now = maximum_broadcast;
	assert!(should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::Pending {
			maximum_broadcast,
			clock_drift: 0,
			considered: 10,
			next_no_show: None,
		},
		tranche_now,
	));
}

#[test]
fn assignment_not_triggered_if_at_maximum_but_clock_is_before() {
	let block_hash = Hash::repeat_byte(0x01);
	let maximum_broadcast = 10;

	let candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: maximum_broadcast,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	let tranche_now = 9;
	assert!(!should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::Pending {
			maximum_broadcast,
			clock_drift: 0,
			considered: 10,
			next_no_show: None,
		},
		tranche_now,
	));
}

#[test]
fn assignment_not_triggered_if_at_maximum_but_clock_is_before_with_drift() {
	let block_hash = Hash::repeat_byte(0x01);
	let maximum_broadcast = 10;

	let candidate_entry: CandidateEntry = {
		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			backing_group: GroupIndex(0),
			our_assignment: Some(approval_db::v1::OurAssignment {
				cert: garbage_assignment_cert(
					AssignmentCertKind::RelayVRFModulo { sample: 0 }
				),
				tranche: maximum_broadcast,
				validator_index: ValidatorIndex(4),
				triggered: false,
			}),
			assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
			approved: false,
		};

		approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 1,
			block_assignments: vec![(block_hash, approval_entry)].into_iter().collect(),
			approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; 4],
		}.into()
	};

	let tranche_now = 10;
	assert!(!should_trigger_assignment(
		candidate_entry.approval_entry(&block_hash).unwrap(),
		&candidate_entry,
		RequiredTranches::Pending {
			maximum_broadcast,
			clock_drift: 1,
			considered: 10,
			next_no_show: None,
		},
		tranche_now,
	));
}

#[test]
fn wakeups_next() {
	let mut wakeups = Wakeups::default();

	let b_a = Hash::repeat_byte(0);
	let b_b = Hash::repeat_byte(1);

	let c_a = CandidateHash(Hash::repeat_byte(2));
	let c_b = CandidateHash(Hash::repeat_byte(3));

	wakeups.schedule(b_a, c_a, 1);
	wakeups.schedule(b_a, c_b, 4);
	wakeups.schedule(b_b, c_b, 3);

	assert_eq!(wakeups.first().unwrap(), 1);

	let clock = MockClock::new(0);
	let clock_aux = clock.clone();

	let test_fut = Box::pin(async move {
		assert_eq!(wakeups.next(&clock).await, (1, b_a, c_a));
		assert_eq!(wakeups.next(&clock).await, (3, b_b, c_b));
		assert_eq!(wakeups.next(&clock).await, (4, b_a, c_b));
		assert!(wakeups.first().is_none());
		assert!(wakeups.wakeups.is_empty());
	});

	let aux_fut = Box::pin(async move {
		clock_aux.inner.lock().set_tick(1);
		// skip direct set to 3.
		clock_aux.inner.lock().set_tick(4);
	});

	futures::executor::block_on(futures::future::join(test_fut, aux_fut));
}

#[test]
fn wakeup_earlier_supersedes_later() {
	let mut wakeups = Wakeups::default();

	let b_a = Hash::repeat_byte(0);
	let c_a = CandidateHash(Hash::repeat_byte(2));

	wakeups.schedule(b_a, c_a, 4);
	wakeups.schedule(b_a, c_a, 2);
	wakeups.schedule(b_a, c_a, 3);

	let clock = MockClock::new(0);
	let clock_aux = clock.clone();

	let test_fut = Box::pin(async move {
		assert_eq!(wakeups.next(&clock).await, (2, b_a, c_a));
		assert!(wakeups.first().is_none());
		assert!(wakeups.reverse_wakeups.is_empty());
	});

	let aux_fut = Box::pin(async move {
		clock_aux.inner.lock().set_tick(2);
	});

	futures::executor::block_on(futures::future::join(test_fut, aux_fut));
}

#[test]
fn block_not_approved_until_all_candidates_approved() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let candidate_hash_2 = CandidateHash(Hash::repeat_byte(0xDD));

	let validator_index_a = ValidatorIndex(0);
	let validator_index_b = ValidatorIndex(1);

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![ValidatorIndex(0), ValidatorIndex(1)], vec![ValidatorIndex(2)]],
			needed_approvals: 2,
			..Default::default()
		})
	};

	add_candidate_to_block(
		&mut state.db,
		block_hash,
		candidate_hash_2,
		3,
		CoreIndex(1),
		GroupIndex(1),
	);

	let approve_candidate = |db: &mut TestStore, c_hash| {
		db.candidate_entries.get_mut(&c_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_a, 0);

		db.candidate_entries.get_mut(&c_hash).unwrap()
			.approval_entry_mut(&block_hash)
			.unwrap()
			.import_assignment(0, validator_index_b, 0);

		assert!(!db.candidate_entries.get_mut(&c_hash).unwrap()
			.mark_approval(validator_index_a));

		assert!(!db.candidate_entries.get_mut(&c_hash).unwrap()
			.mark_approval(validator_index_b));
	};

	approve_candidate(&mut state.db, candidate_hash_2);

	{
		let b = state.db.block_entries.get_mut(&block_hash).unwrap();
		b.mark_approved_by_hash(&candidate_hash);
		assert!(!b.is_fully_approved());
	}

	let actions = check_and_apply_full_approval(
		&state,
		None,
		candidate_hash_2,
		state.db.candidate_entries.get(&candidate_hash_2).unwrap().clone(),
		|b_hash, _a| b_hash == &block_hash,
	).unwrap();

	assert_eq!(actions.len(), 2);
	assert_matches!(
		actions.get(0).unwrap(),
		Action::WriteBlockEntry(b_entry) => {
			assert_eq!(b_entry.block_hash(), block_hash);
			assert!(b_entry.is_fully_approved());
			assert!(b_entry.is_candidate_approved(&candidate_hash_2));
		}
	);

	assert_matches!(
		actions.get(1).unwrap(),
		Action::WriteCandidateEntry(c_h, c_entry) => {
			assert_eq!(c_h, &candidate_hash_2);
			assert!(c_entry.approval_entry(&block_hash).unwrap().is_approved());
		}
	);
}

#[test]
fn candidate_approval_applied_to_all_blocks() {
	let block_hash = Hash::repeat_byte(0x01);
	let block_hash_2 = Hash::repeat_byte(0x02);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let validator_index_a = ValidatorIndex(0);
	let validator_index_b = ValidatorIndex(1);

	let slot = Slot::from(1);
	let session_index = 1;

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			validator_groups: vec![vec![ValidatorIndex(0), ValidatorIndex(1)], vec![ValidatorIndex(2)]],
			needed_approvals: 2,
			session_index,
			slot,
			..Default::default()
		})
	};

	add_block(
		&mut state.db,
		block_hash_2,
		session_index,
		slot,
	);

	add_candidate_to_block(
		&mut state.db,
		block_hash_2,
		candidate_hash,
		3,
		CoreIndex(1),
		GroupIndex(1),
	);

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_a, 0);

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.import_assignment(0, validator_index_b, 0);

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash_2)
		.unwrap()
		.import_assignment(0, validator_index_a, 0);

	state.db.candidate_entries.get_mut(&candidate_hash).unwrap()
		.approval_entry_mut(&block_hash_2)
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
		|_b_hash, _a| true,
	).unwrap();

	assert_eq!(actions.len(), 3);
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
		Action::WriteBlockEntry(b_entry) => {
			assert_eq!(b_entry.block_hash(), block_hash_2);
			assert!(b_entry.is_fully_approved());
			assert!(b_entry.is_candidate_approved(&candidate_hash));
		}
	);
	assert_matches!(
		actions.get(2).unwrap(),
		Action::WriteCandidateEntry(c_hash, c_entry) => {
			assert_eq!(c_hash, &candidate_hash);
			assert!(c_entry.approval_entry(&block_hash).unwrap().is_approved());
			assert!(c_entry.approval_entry(&block_hash_2).unwrap().is_approved());
		}
	);
}

#[test]
fn approved_ancestor_all_approved() {
	let block_hash_1 = Hash::repeat_byte(0x01);
	let block_hash_2 = Hash::repeat_byte(0x02);
	let block_hash_3 = Hash::repeat_byte(0x03);
	let block_hash_4 = Hash::repeat_byte(0x04);

	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let slot = Slot::from(1);
	let session_index = 1;

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob],
			validator_groups: vec![vec![ValidatorIndex(0)], vec![ValidatorIndex(1)]],
			needed_approvals: 2,
			session_index,
			slot,
			..Default::default()
		})
	};

	let add_block = |db: &mut TestStore, block_hash, approved| {
		add_block(
			db,
			block_hash,
			session_index,
			slot,
		);

		let b = db.block_entries.get_mut(&block_hash).unwrap();
		b.add_candidate(CoreIndex(0), candidate_hash);
		if approved {
			b.mark_approved_by_hash(&candidate_hash);
		}
	};

	add_block(&mut state.db, block_hash_1, true);
	add_block(&mut state.db, block_hash_2, true);
	add_block(&mut state.db, block_hash_3, true);
	add_block(&mut state.db, block_hash_4, true);

	let pool = TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

	let test_fut = Box::pin(async move {
		assert_eq!(
			handle_approved_ancestor(&mut ctx, &state.db, block_hash_4, 0).await.unwrap(),
			Some((block_hash_4, 4)),
		)
	});

	let aux_fut = Box::pin(async move {
		assert_matches!(
			handle.recv().await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(target, tx)) => {
				assert_eq!(target, block_hash_4);
				let _ = tx.send(Ok(Some(4)));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::ChainApi(ChainApiMessage::Ancestors {
				hash,
				k,
				response_channel: tx,
			}) => {
				assert_eq!(hash, block_hash_4);
				assert_eq!(k, 4 - (0 + 1));
				let _ = tx.send(Ok(vec![block_hash_3, block_hash_2, block_hash_1]));
			}
		);
	});

	futures::executor::block_on(futures::future::join(test_fut, aux_fut));
}

#[test]
fn approved_ancestor_missing_approval() {
	let block_hash_1 = Hash::repeat_byte(0x01);
	let block_hash_2 = Hash::repeat_byte(0x02);
	let block_hash_3 = Hash::repeat_byte(0x03);
	let block_hash_4 = Hash::repeat_byte(0x04);

	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));

	let slot = Slot::from(1);
	let session_index = 1;

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob],
			validator_groups: vec![vec![ValidatorIndex(0)], vec![ValidatorIndex(1)]],
			needed_approvals: 2,
			session_index,
			slot,
			..Default::default()
		})
	};

	let add_block = |db: &mut TestStore, block_hash, approved| {
		add_block(
			db,
			block_hash,
			session_index,
			slot,
		);

		let b = db.block_entries.get_mut(&block_hash).unwrap();
		b.add_candidate(CoreIndex(0), candidate_hash);
		if approved {
			b.mark_approved_by_hash(&candidate_hash);
		}
	};

	add_block(&mut state.db, block_hash_1, true);
	add_block(&mut state.db, block_hash_2, true);
	add_block(&mut state.db, block_hash_3, false);
	add_block(&mut state.db, block_hash_4, true);

	let pool = TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

	let test_fut = Box::pin(async move {
		assert_eq!(
			handle_approved_ancestor(&mut ctx, &state.db, block_hash_4, 0).await.unwrap(),
			Some((block_hash_2, 2)),
		)
	});

	let aux_fut = Box::pin(async move {
		assert_matches!(
			handle.recv().await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(target, tx)) => {
				assert_eq!(target, block_hash_4);
				let _ = tx.send(Ok(Some(4)));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::ChainApi(ChainApiMessage::Ancestors {
				hash,
				k,
				response_channel: tx,
			}) => {
				assert_eq!(hash, block_hash_4);
				assert_eq!(k, 4 - (0 + 1));
				let _ = tx.send(Ok(vec![block_hash_3, block_hash_2, block_hash_1]));
			}
		);
	});

	futures::executor::block_on(futures::future::join(test_fut, aux_fut));
}

#[test]
fn process_wakeup_trigger_assignment_launch_approval() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let slot = Slot::from(1);
	let session_index = 1;

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(0)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob],
			validator_groups: vec![vec![ValidatorIndex(0)], vec![ValidatorIndex(1)]],
			needed_approvals: 2,
			session_index,
			slot,
			..Default::default()
		})
	};

	let actions = process_wakeup(
		&state,
		block_hash,
		candidate_hash,
		1,
	).unwrap();

	assert!(actions.is_empty());

	state.db.candidate_entries
		.get_mut(&candidate_hash)
		.unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.set_our_assignment(approval_db::v1::OurAssignment {
			cert: garbage_assignment_cert(
				AssignmentCertKind::RelayVRFModulo { sample: 0 }
			),
			tranche: 0,
			validator_index: ValidatorIndex(0),
			triggered: false,
		}.into());

	let actions = process_wakeup(
		&state,
		block_hash,
		candidate_hash,
		1,
	).unwrap();

	assert_eq!(actions.len(), 3);
	assert_matches!(
		actions.get(0).unwrap(),
		Action::WriteCandidateEntry(c_hash, c_entry) => {
			assert_eq!(c_hash, &candidate_hash);
			assert!(c_entry
				.approval_entry(&block_hash)
				.unwrap()
				.our_assignment()
				.unwrap()
				.triggered()
			);
		}
	);

	assert_matches!(
		actions.get(1).unwrap(),
		Action::LaunchApproval {
			candidate_index,
			..
		} => {
			assert_eq!(candidate_index, &0);
		}
	);

	assert_matches!(
		actions.get(2).unwrap(),
		Action::ScheduleWakeup {
			tick,
			..
		} => {
			assert_eq!(tick, &slot_to_tick(0 + 2));
		}
	)
}

#[test]
fn process_wakeup_schedules_wakeup() {
	let block_hash = Hash::repeat_byte(0x01);
	let candidate_hash = CandidateHash(Hash::repeat_byte(0xCC));
	let slot = Slot::from(1);
	let session_index = 1;

	let mut state = State {
		assignment_criteria: Box::new(MockAssignmentCriteria::check_only(|| {
			Ok(10)
		})),
		..some_state(StateConfig {
			validators: vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob],
			validator_groups: vec![vec![ValidatorIndex(0)], vec![ValidatorIndex(1)]],
			needed_approvals: 2,
			session_index,
			slot,
			..Default::default()
		})
	};

	state.db.candidate_entries
		.get_mut(&candidate_hash)
		.unwrap()
		.approval_entry_mut(&block_hash)
		.unwrap()
		.set_our_assignment(approval_db::v1::OurAssignment {
			cert: garbage_assignment_cert(
				AssignmentCertKind::RelayVRFModulo { sample: 0 }
			),
			tranche: 10,
			validator_index: ValidatorIndex(0),
			triggered: false,
		}.into());

	let actions = process_wakeup(
		&state,
		block_hash,
		candidate_hash,
		1,
	).unwrap();

	assert_eq!(actions.len(), 1);
	assert_matches!(
		actions.get(0).unwrap(),
		Action::ScheduleWakeup { block_hash: b, candidate_hash: c, tick } => {
			assert_eq!(b, &block_hash);
			assert_eq!(c, &candidate_hash);
			assert_eq!(tick, &(slot_to_tick(slot) + 10));
		}
	);
}

#[test]
fn triggered_assignment_leads_to_recovery_and_validation() {

}

#[test]
fn finalization_event_prunes() {

}
