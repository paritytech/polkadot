#![allow(dead_code)]
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
use polkadot_node_primitives::approval::{
	AssignmentCert, AssignmentCertKind, DelayTranche, VRFOutput, VRFProof, RELAY_VRF_MODULO_CONTEXT,
};
use polkadot_node_subsystem::{
	messages::{AllMessages, ApprovalVotingMessage, AssignmentCheckResult},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_overseer::HeadSupportsParachains;
use polkadot_primitives::v1::{CandidateEvent, CoreIndex, GroupIndex, Header, ValidatorSignature};
use std::time::Duration;

use assert_matches::assert_matches;
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

use super::{
	approval_db::v1::StoredBlockRange,
	backend::BackendWriteOp,
	import::tests::{
		garbage_vrf, AllowedSlots, BabeEpoch, BabeEpochConfiguration, CompatibleDigestItem, Digest,
		DigestItem, PreDigest, SecondaryVRFPreDigest,
	},
};

const SLOT_DURATION_MILLIS: u64 = 5000;

#[derive(Clone)]
struct TestSyncOracle {
	flag: Arc<AtomicBool>,
	done_syncing_sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

struct TestSyncOracleHandle {
	done_syncing_receiver: oneshot::Receiver<()>,
	flag: Arc<AtomicBool>,
}

impl TestSyncOracleHandle {
	fn set_done(&self) {
		self.flag.store(false, Ordering::SeqCst);
	}

	async fn await_mode_switch(self) {
		let _ = self.done_syncing_receiver.await;
	}
}

impl SyncOracle for TestSyncOracle {
	fn is_major_syncing(&mut self) -> bool {
		let is_major_syncing = self.flag.load(Ordering::SeqCst);

		if !is_major_syncing {
			if let Some(sender) = self.done_syncing_sender.lock().take() {
				let _ = sender.send(());
			}
		}

		is_major_syncing
	}

	fn is_offline(&mut self) -> bool {
		unimplemented!("not used in network bridge")
	}
}

// val - result of `is_major_syncing`.
fn make_sync_oracle(val: bool) -> (TestSyncOracle, TestSyncOracleHandle) {
	let (tx, rx) = oneshot::channel();
	let flag = Arc::new(AtomicBool::new(val));

	(
		TestSyncOracle { flag: flag.clone(), done_syncing_sender: Arc::new(Mutex::new(Some(tx))) },
		TestSyncOracleHandle { flag, done_syncing_receiver: rx },
	)
}

fn done_syncing_oracle() -> Box<dyn SyncOracle + Send> {
	let (oracle, _) = make_sync_oracle(false);
	Box::new(oracle)
}

#[cfg(test)]
pub mod test_constants {
	use crate::approval_db::v1::Config as DatabaseConfig;
	const DATA_COL: u32 = 0;
	pub(crate) const NUM_COLUMNS: u32 = 1;

	pub(crate) const TEST_CONFIG: DatabaseConfig = DatabaseConfig { col_data: DATA_COL };
}

struct MockSupportsParachains;

impl HeadSupportsParachains for MockSupportsParachains {
	fn head_supports_parachains(&self, _head: &Hash) -> bool {
		true
	}
}

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
		let drain_up_to =
			self.wakeups.binary_search_by_key(&(up_to + 1), |w| w.0).unwrap_or_else(|i| i);

		for (_, wakeup) in self.wakeups.drain(..drain_up_to) {
			let _ = wakeup.send(());
		}
	}

	fn has_wakeup(&self, tick: Tick) -> bool {
		self.wakeups.binary_search_by_key(&tick, |w| w.0).is_ok()
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

		let pos = self.wakeups.binary_search_by_key(&tick, |w| w.0).unwrap_or_else(|i| i);

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
	Check: Fn() -> Result<DelayTranche, criteria::InvalidAssignment>,
{
	fn compute_assignments(
		&self,
		_keystore: &LocalKeystore,
		_relay_vrf_story: polkadot_node_primitives::approval::RelayVRFStory,
		_config: &criteria::Config,
		_leaving_cores: Vec<(
			CandidateHash,
			polkadot_primitives::v1::CoreIndex,
			polkadot_primitives::v1::GroupIndex,
		)>,
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

impl<F>
	MockAssignmentCriteria<
		fn() -> HashMap<polkadot_primitives::v1::CoreIndex, criteria::OurAssignment>,
		F,
	>
{
	fn check_only(f: F) -> Self {
		MockAssignmentCriteria(Default::default, f)
	}
}

#[derive(Default)]
struct TestStore {
	stored_block_range: Option<StoredBlockRange>,
	blocks_at_height: HashMap<BlockNumber, Vec<Hash>>,
	block_entries: HashMap<Hash, BlockEntry>,
	candidate_entries: HashMap<CandidateHash, CandidateEntry>,
}

impl Backend for TestStore {
	fn load_block_entry(&self, block_hash: &Hash) -> SubsystemResult<Option<BlockEntry>> {
		Ok(self.block_entries.get(block_hash).cloned())
	}

	fn load_candidate_entry(
		&self,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateEntry>> {
		Ok(self.candidate_entries.get(candidate_hash).cloned())
	}

	fn load_blocks_at_height(&self, height: &BlockNumber) -> SubsystemResult<Vec<Hash>> {
		Ok(self.blocks_at_height.get(height).cloned().unwrap_or_default())
	}

	fn load_all_blocks(&self) -> SubsystemResult<Vec<Hash>> {
		let mut hashes: Vec<_> = self.block_entries.keys().cloned().collect();

		hashes.sort_by_key(|k| self.block_entries.get(k).unwrap().block_number());

		Ok(hashes)
	}

	fn load_stored_blocks(&self) -> SubsystemResult<Option<StoredBlockRange>> {
		Ok(self.stored_block_range.clone())
	}

	fn write<I>(&mut self, ops: I) -> SubsystemResult<()>
	where
		I: IntoIterator<Item = BackendWriteOp>,
	{
		for op in ops {
			match op {
				BackendWriteOp::WriteStoredBlockRange(stored_block_range) => {
					self.stored_block_range = Some(stored_block_range);
				},
				BackendWriteOp::WriteBlocksAtHeight(h, blocks) => {
					self.blocks_at_height.insert(h, blocks);
				},
				BackendWriteOp::DeleteBlocksAtHeight(h) => {
					let _ = self.blocks_at_height.remove(&h);
				},
				BackendWriteOp::WriteBlockEntry(block_entry) => {
					self.block_entries.insert(block_entry.block_hash(), block_entry);
				},
				BackendWriteOp::DeleteBlockEntry(hash) => {
					let _ = self.block_entries.remove(&hash);
				},
				BackendWriteOp::WriteCandidateEntry(candidate_entry) => {
					self.candidate_entries
						.insert(candidate_entry.candidate_receipt().hash(), candidate_entry);
				},
				BackendWriteOp::DeleteCandidateEntry(candidate_hash) => {
					let _ = self.candidate_entries.remove(&candidate_hash);
				},
			}
		}

		Ok(())
	}
}

fn garbage_assignment_cert(kind: AssignmentCertKind) -> AssignmentCert {
	let ctx = schnorrkel::signing_context(RELAY_VRF_MODULO_CONTEXT);
	let msg = b"test-garbage";
	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);
	let (inout, proof, _) = keypair.vrf_sign(ctx.bytes(msg));
	let out = inout.to_output();

	AssignmentCert { kind, vrf: (VRFOutput(out), VRFProof(proof)) }
}

fn sign_approval(
	key: Sr25519Keyring,
	candidate_hash: CandidateHash,
	session_index: SessionIndex,
) -> ValidatorSignature {
	key.sign(&ApprovalVote(candidate_hash).signing_payload(session_index)).into()
}

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ApprovalVotingMessage>;

struct TestHarness {
	virtual_overseer: VirtualOverseer,
	clock: Box<MockClock>,
}

#[derive(Default)]
struct HarnessConfig {
	tick_start: Tick,
	assigned_tranche: DelayTranche,
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	config: HarnessConfig,
	sync_oracle: Box<dyn SyncOracle + Send>,
	test: impl FnOnce(TestHarness) -> T,
) {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool);

	let keystore = LocalKeystore::in_memory();
	let _ = keystore.sr25519_generate_new(
		polkadot_primitives::v1::PARACHAIN_KEY_TYPE_ID,
		Some(&Sr25519Keyring::Alice.to_seed()),
	);

	let store = TestStore::default();

	let HarnessConfig { tick_start, assigned_tranche } = config;

	let clock = Box::new(MockClock::new(tick_start));
	let subsystem = run(
		context,
		ApprovalVotingSubsystem::with_config(
			Config { col_data: test_constants::TEST_CONFIG.col_data, slot_duration_millis: 100u64 },
			Arc::new(kvdb_memorydb::create(test_constants::NUM_COLUMNS)),
			Arc::new(keystore),
			sync_oracle,
			Metrics::default(),
		),
		clock.clone(),
		Box::new(MockAssignmentCriteria::check_only(move || Ok(assigned_tranche))),
		store,
	);

	let test_fut = test(TestHarness { virtual_overseer, clock });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	futures::executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		},
		subsystem,
	))
	.1
	.unwrap();
}

async fn overseer_send(overseer: &mut VirtualOverseer, msg: FromOverseer<ApprovalVotingMessage>) {
	tracing::trace!("Sending message:\n{:?}", &msg);
	overseer
		.send(msg)
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

	tracing::trace!("Received message:\n{:?}", &msg);

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut VirtualOverseer,
	timeout: Duration,
) -> Option<AllMessages> {
	tracing::trace!("Waiting for message...");
	overseer.recv().timeout(timeout).await
}

const TIMEOUT: Duration = Duration::from_millis(2000);
async fn overseer_signal(overseer: &mut VirtualOverseer, signal: OverseerSignal) {
	overseer
		.send(FromOverseer::Signal(signal))
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
}

#[test]
fn blank_subsystem_act_on_bad_block() {
	let (oracle, handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let (tx, rx) = oneshot::channel();

		let bad_block_hash: Hash = Default::default();

		overseer_send(
			&mut virtual_overseer,
			FromOverseer::Communication {
				msg: ApprovalVotingMessage::CheckAndImportAssignment(
					IndirectAssignmentCert {
						block_hash: bad_block_hash.clone(),
						validator: 0u32.into(),
						cert: garbage_assignment_cert(AssignmentCertKind::RelayVRFModulo {
							sample: 0,
						}),
					},
					0u32,
					tx,
				),
			},
		)
		.await;

		handle.await_mode_switch().await;

		assert_matches!(
			rx.await,
			Ok(
				AssignmentCheckResult::Bad(AssignmentCheckError::UnknownBlock(hash))
			) => {
				assert_eq!(hash, bad_block_hash);
			}
		);

		virtual_overseer
	});
}

#[test]
fn ss_rejects_approval_if_no_block_entry() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let block_hash = Hash::repeat_byte(0x01);
		let candidate_index = 0;
		let validator = ValidatorIndex(0);
		let candidate_hash = CandidateReceipt::<Hash>::default().hash();
		let session_index = 1;

		let rx = cai_approval(
			&mut virtual_overseer,
			block_hash,
			candidate_index,
			validator,
			candidate_hash,
			session_index,
			false,
		)
		.await;

		assert_matches!(
			rx.await,
			Ok(ApprovalCheckResult::Bad(ApprovalCheckError::UnknownBlock(hash))) => {
				assert_eq!(hash, block_hash);
			}
		);

		virtual_overseer
	});
}

#[test]
fn ss_rejects_approval_before_assignment() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let block_hash = Hash::repeat_byte(0x01);

		let candidate_hash = {
			let mut candidate_receipt = CandidateReceipt::<Hash>::default();
			candidate_receipt.descriptor.para_id = 1.into();
			candidate_receipt.descriptor.relay_parent = block_hash;
			candidate_receipt.hash()
		};

		let candidate_index = 0;
		let validator = ValidatorIndex(0);
		let session_index = 1;

		// Add block hash 00.
		ChainBuilder::new()
			.add_block(block_hash, ChainBuilder::GENESIS_HASH, Slot::from(1), 1)
			.build(&mut virtual_overseer)
			.await;

		let rx = cai_approval(
			&mut virtual_overseer,
			block_hash,
			candidate_index,
			validator,
			candidate_hash,
			session_index,
			false,
		)
		.await;

		assert_matches!(
			rx.await,
			Ok(ApprovalCheckResult::Bad(ApprovalCheckError::NoAssignment(v))) => {
				assert_eq!(v, validator);
			}
		);

		virtual_overseer
	});
}

#[test]
fn ss_rejects_assignment_in_future() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(
		HarnessConfig {
			tick_start: 0,
			assigned_tranche: TICK_TOO_FAR_IN_FUTURE as _,
			..Default::default()
		},
		Box::new(oracle),
		|test_harness| async move {
			let TestHarness { mut virtual_overseer, clock } = test_harness;

			let block_hash = Hash::repeat_byte(0x01);
			let candidate_index = 0;
			let validator = ValidatorIndex(0);

			// Add block hash 00.
			ChainBuilder::new()
				.add_block(block_hash, ChainBuilder::GENESIS_HASH, Slot::from(1), 1)
				.build(&mut virtual_overseer)
				.await;

			let rx =
				cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

			assert_eq!(rx.await, Ok(AssignmentCheckResult::TooFarInFuture));

			// Advance clock to make assignment reasonably near.
			clock.inner.lock().set_tick(1);

			let rx =
				cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

			assert_eq!(rx.await, Ok(AssignmentCheckResult::Accepted));

			virtual_overseer
		},
	);
}

#[test]
fn ss_accepts_duplicate_assignment() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let block_hash = Hash::repeat_byte(0x01);
		let candidate_index = 0;
		let validator = ValidatorIndex(0);

		// Add block hash 00.
		ChainBuilder::new()
			.add_block(block_hash, ChainBuilder::GENESIS_HASH, Slot::from(1), 1)
			.build(&mut virtual_overseer)
			.await;

		let rx =
			cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

		assert_eq!(rx.await, Ok(AssignmentCheckResult::Accepted));

		let rx =
			cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

		assert_eq!(rx.await, Ok(AssignmentCheckResult::AcceptedDuplicate));

		virtual_overseer
	});
}

#[test]
fn ss_rejects_assignment_with_unknown_candidate() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let block_hash = Hash::repeat_byte(0x01);
		let candidate_index = 7;
		let validator = ValidatorIndex(0);

		// Add block hash 00.
		ChainBuilder::new()
			.add_block(block_hash, ChainBuilder::GENESIS_HASH, Slot::from(1), 1)
			.build(&mut virtual_overseer)
			.await;

		let rx =
			cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

		assert_eq!(
			rx.await,
			Ok(AssignmentCheckResult::Bad(AssignmentCheckError::InvalidCandidateIndex(
				candidate_index
			))),
		);

		virtual_overseer
	});
}

#[test]
fn ss_accepts_and_imports_approval_after_assignment() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let block_hash = Hash::repeat_byte(0x01);

		let candidate_hash = {
			let mut candidate_receipt = CandidateReceipt::<Hash>::default();
			candidate_receipt.descriptor.para_id = 1.into();
			candidate_receipt.descriptor.relay_parent = block_hash;
			candidate_receipt.hash()
		};

		let candidate_index = 0;
		let validator = ValidatorIndex(0);
		let session_index = 1;

		// Add block hash 0x01...
		ChainBuilder::new()
			.add_block(block_hash, ChainBuilder::GENESIS_HASH, Slot::from(1), 1)
			.build(&mut virtual_overseer)
			.await;

		let rx =
			cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

		assert_eq!(rx.await, Ok(AssignmentCheckResult::Accepted));

		let rx = cai_approval(
			&mut virtual_overseer,
			block_hash,
			candidate_index,
			validator,
			candidate_hash,
			session_index,
			true,
		)
		.await;

		assert_eq!(rx.await, Ok(ApprovalCheckResult::Accepted));

		virtual_overseer
	});
}
#[test]
fn ss_assignment_import_updates_candidate_entry_and_schedules_wakeup() {
	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let block_hash = Hash::repeat_byte(0x01);

		let _candidate_hash = {
			let mut candidate_receipt = CandidateReceipt::<Hash>::default();
			candidate_receipt.descriptor.para_id = 1.into();
			candidate_receipt.descriptor.relay_parent = block_hash;
			candidate_receipt.hash()
		};

		let candidate_index = 0;
		let validator = ValidatorIndex(0);

		// Add block hash 0x01...
		ChainBuilder::new()
			.add_block(block_hash, ChainBuilder::GENESIS_HASH, Slot::from(1), 1)
			.build(&mut virtual_overseer)
			.await;

		let rx =
			cai_assignment(&mut virtual_overseer, block_hash, candidate_index, validator).await;

		assert_eq!(rx.await, Ok(AssignmentCheckResult::Accepted));

		// TODO(ladi): fix
		//assert!(clock.inner.lock().has_wakeup(20));

		virtual_overseer
	});
}

async fn cai_approval(
	overseer: &mut VirtualOverseer,
	block_hash: Hash,
	candidate_index: CandidateIndex,
	validator: ValidatorIndex,
	candidate_hash: CandidateHash,
	session_index: SessionIndex,
	expect_coordinator: bool,
) -> oneshot::Receiver<ApprovalCheckResult> {
	let signature = sign_approval(Sr25519Keyring::Alice, candidate_hash, session_index);
	let (tx, rx) = oneshot::channel();
	overseer_send(
		overseer,
		FromOverseer::Communication {
			msg: ApprovalVotingMessage::CheckAndImportApproval(
				IndirectSignedApprovalVote { block_hash, candidate_index, validator, signature },
				tx,
			),
		},
	)
	.await;

	if expect_coordinator {
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::ImportStatements {
				pending_confirmation,
				..
			}) => {
				let _ = pending_confirmation.send(ImportStatementsResult::ValidImport);
			}
		);
	}
	rx
}

async fn cai_assignment(
	overseer: &mut VirtualOverseer,
	block_hash: Hash,
	candidate_index: CandidateIndex,
	validator: ValidatorIndex,
) -> oneshot::Receiver<AssignmentCheckResult> {
	let (tx, rx) = oneshot::channel();
	overseer_send(
		overseer,
		FromOverseer::Communication {
			msg: ApprovalVotingMessage::CheckAndImportAssignment(
				IndirectAssignmentCert {
					block_hash,
					validator,
					cert: garbage_assignment_cert(AssignmentCertKind::RelayVRFModulo { sample: 0 }),
				},
				candidate_index,
				tx,
			),
		},
	)
	.await;
	rx
}

struct ChainBuilder {
	blocks_by_hash: HashMap<Hash, Header>,
	blocks_at_height: BTreeMap<u32, Vec<Hash>>,
}

impl ChainBuilder {
	const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);
	const GENESIS_PARENT_HASH: Hash = Hash::repeat_byte(0x00);

	pub fn new() -> Self {
		let mut builder =
			Self { blocks_by_hash: HashMap::new(), blocks_at_height: BTreeMap::new() };
		builder.add_block_inner(Self::GENESIS_HASH, Self::GENESIS_PARENT_HASH, Slot::from(0), 0);
		builder
	}

	pub fn add_block<'a>(
		&'a mut self,
		hash: Hash,
		parent_hash: Hash,
		slot: Slot,
		number: u32,
	) -> &'a mut Self {
		assert!(number != 0, "cannot add duplicate genesis block");
		assert!(hash != Self::GENESIS_HASH, "cannot add block with genesis hash");
		assert!(
			parent_hash != Self::GENESIS_PARENT_HASH,
			"cannot add block with genesis parent hash"
		);
		assert!(self.blocks_by_hash.len() < u8::MAX.into());
		self.add_block_inner(hash, parent_hash, slot, number)
	}

	fn add_block_inner<'a>(
		&'a mut self,
		hash: Hash,
		parent_hash: Hash,
		slot: Slot,
		number: u32,
	) -> &'a mut Self {
		let header = ChainBuilder::make_header(parent_hash, slot, number);
		assert!(
			self.blocks_by_hash.insert(hash, header).is_none(),
			"block with hash {:?} already exists",
			hash,
		);
		self.blocks_at_height.entry(number).or_insert_with(Vec::new).push(hash);
		self
	}

	pub async fn build(&self, overseer: &mut VirtualOverseer) {
		for (number, blocks) in self.blocks_at_height.iter() {
			for (i, hash) in blocks.iter().enumerate() {
				let mut cur_hash = *hash;
				let mut ancestry = Vec::new();
				while cur_hash != Self::GENESIS_PARENT_HASH {
					let cur_header =
						self.blocks_by_hash.get(&cur_hash).expect("chain is not contiguous");
					ancestry.push((cur_hash, cur_header.clone()));
					cur_hash = cur_header.parent_hash;
				}
				ancestry.reverse();
				import_block(overseer, ancestry.as_ref(), *number, false, i > 0).await;
				let _: Option<()> = future::pending().timeout(Duration::from_millis(100)).await;
			}
		}
	}

	fn make_header(parent_hash: Hash, slot: Slot, number: u32) -> Header {
		let digest = {
			let mut digest = Digest::default();
			let (vrf_output, vrf_proof) = garbage_vrf();
			digest.push(DigestItem::babe_pre_digest(PreDigest::SecondaryVRF(
				SecondaryVRFPreDigest { authority_index: 0, slot, vrf_output, vrf_proof },
			)));
			digest
		};

		Header {
			digest,
			extrinsics_root: Default::default(),
			number,
			state_root: Default::default(),
			parent_hash,
		}
	}
}

async fn import_block(
	overseer: &mut VirtualOverseer,
	hashes: &[(Hash, Header)],
	session: u32,
	gap: bool,
	fork: bool,
) {
	let validators = vec![Sr25519Keyring::Alice, Sr25519Keyring::Bob];
	let session_info = SessionInfo {
		validators: validators.iter().map(|v| v.public().into()).collect(),
		discovery_keys: validators.iter().map(|v| v.public().into()).collect(),
		assignment_keys: validators.iter().map(|v| v.public().into()).collect(),
		validator_groups: vec![vec![ValidatorIndex(0)], vec![ValidatorIndex(1)]],
		n_cores: 6,
		needed_approvals: 1,
		zeroth_delay_tranche_width: 5,
		relay_vrf_modulo_samples: 3,
		n_delay_tranches: 50,
		no_show_slots: 2,
	};

	let (new_head, new_header) = &hashes[hashes.len() - 1];
	overseer_send(
		overseer,
		FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			ActivatedLeaf {
				hash: *new_head,
				number: session,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			},
		))),
	)
	.await;

	assert_matches!(
		overseer_recv(overseer).await,
		AllMessages::ChainApi(ChainApiMessage::BlockHeader(head, h_tx)) => {
			assert_eq!(*new_head, head);
			h_tx.send(Ok(Some(new_header.clone()))).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(overseer).await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(
				req_block_hash,
				RuntimeApiRequest::SessionIndexForChild(s_tx)
			)
		) => {
			let hash = &hashes[session.saturating_sub(1) as usize];
			assert_eq!(req_block_hash, hash.0.clone());
			s_tx.send(Ok(session.into())).unwrap();
		}
	);

	if !fork {
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(
					req_block_hash,
					RuntimeApiRequest::SessionInfo(idx, si_tx),
				)
			) => {
				assert_eq!(session, idx);
				assert_eq!(req_block_hash, *new_head);
				si_tx.send(Ok(Some(session_info.clone()))).unwrap();
			}
		);

		let mut _ancestry_step = 0;
		if gap {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash,
					k,
					response_channel,
				}) => {
					assert_eq!(hash, *new_head);
					let history: Vec<Hash> = hashes.iter().map(|v| v.0).take(k).collect();
					let _ = response_channel.send(Ok(history));
					_ancestry_step = k;
				}
			);

			for i in 0.._ancestry_step {
				match overseer_recv(overseer).await {
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(_, h_tx)) => {
						let (hash, header) = hashes[i as usize].clone();
						assert_eq!(hash, *new_head);
						h_tx.send(Ok(Some(header))).unwrap();
					},
					AllMessages::ChainApi(ChainApiMessage::Ancestors {
						hash,
						k,
						response_channel,
					}) => {
						assert_eq!(hash, *new_head);
						assert_eq!(k as u32, session - 1);
						let history: Vec<Hash> = hashes.iter().map(|v| v.0).take(k).collect();
						response_channel.send(Ok(history)).unwrap();
					},
					_ => unreachable! {},
				}
			}
		}
	}

	if session > 0 {
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(hash, RuntimeApiRequest::CandidateEvents(c_tx))
			) => {
				assert_eq!(hash, *new_head);

				let make_candidate = |para_id| {
					let mut r = CandidateReceipt::default();
					r.descriptor.para_id = para_id;
					r.descriptor.relay_parent = hash;
					r
				};
				let candidates = vec![
					(make_candidate(1.into()), CoreIndex(0), GroupIndex(2)),
					(make_candidate(2.into()), CoreIndex(1), GroupIndex(3)),
				];

				let inclusion_events = candidates.into_iter()
					.map(|(r, c, g)| CandidateEvent::CandidateIncluded(r, Vec::new().into(), c, g))
					.collect::<Vec<_>>();
				c_tx.send(Ok(inclusion_events)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(
					req_block_hash,
					RuntimeApiRequest::SessionIndexForChild(s_tx)
				)
			) => {
				let hash = &hashes[(session-1) as usize];
				assert_eq!(req_block_hash, hash.0.clone());
				s_tx.send(Ok(session.into())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(
					req_block_hash,
					RuntimeApiRequest::CurrentBabeEpoch(c_tx),
				)
			) => {
				let hash = &hashes[session as usize];
				assert_eq!(req_block_hash, hash.0.clone());
				let _ = c_tx.send(Ok(BabeEpoch {
					epoch_index: session as _,
					start_slot: Slot::from(0),
					duration: 200,
					authorities: vec![(Sr25519Keyring::Alice.public().into(), 1)],
					randomness: [0u8; 32],
					config: BabeEpochConfiguration {
						c: (1, 4),
						allowed_slots: AllowedSlots::PrimarySlots,
					},
				}));
			}
		);
	}

	if session == 0 {
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalDistribution(ApprovalDistributionMessage::NewBlocks(v)) => {
				assert_eq!(v.len(), 0usize);
			}
		);
	} else {
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalDistribution(
				ApprovalDistributionMessage::NewBlocks(mut approval_vec)
			) => {
				assert_eq!(approval_vec.len(), 1);
				let metadata = approval_vec.pop().unwrap();
				let hash = &hashes[session as usize];
				let parent_hash = &hashes[(session - 1) as usize];
				assert_eq!(metadata.hash, hash.0.clone());
				assert_eq!(metadata.parent_hash, parent_hash.0.clone());
				assert_eq!(metadata.slot, Slot::from(session as u64));
			}
		);
	}
}

#[test]
fn linear_import_act_on_leaf() {
	let session = 3u32;

	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let mut head: Hash = ChainBuilder::GENESIS_HASH;
		let mut builder = ChainBuilder::new();
		for i in 1..session {
			let slot = Slot::from(i as u64);

			let hash = Hash::repeat_byte(i as u8);
			builder.add_block(hash, head, slot, i);
			head = hash;
		}

		builder.build(&mut virtual_overseer).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			FromOverseer::Communication {
				msg: ApprovalVotingMessage::CheckAndImportAssignment(
					IndirectAssignmentCert {
						block_hash: head,
						validator: 0u32.into(),
						cert: garbage_assignment_cert(AssignmentCertKind::RelayVRFModulo {
							sample: 0,
						}),
					},
					0u32,
					tx,
				),
			},
		)
		.await;

		assert_eq!(rx.await, Ok(AssignmentCheckResult::Accepted));

		virtual_overseer
	});
}

#[test]
fn forkful_import_at_same_height_act_on_leaf() {
	let session = 3u32;

	let (oracle, _handle) = make_sync_oracle(false);
	test_harness(Default::default(), Box::new(oracle), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let mut head: Hash = ChainBuilder::GENESIS_HASH;
		let mut builder = ChainBuilder::new();
		for i in 1..session {
			let slot = Slot::from(i as u64);
			let hash = Hash::repeat_byte(i as u8);
			builder.add_block(hash, head, slot, i);
			head = hash;
		}
		let num_forks = 3;
		let forks = Vec::new();

		for i in 0..num_forks {
			let slot = Slot::from(session as u64);
			let hash = Hash::repeat_byte(session as u8 + i);
			builder.add_block(hash, head, slot, session);
		}
		builder.build(&mut virtual_overseer).await;

		for head in forks.into_iter() {
			let (tx, rx) = oneshot::channel();

			overseer_send(
				&mut virtual_overseer,
				FromOverseer::Communication {
					msg: ApprovalVotingMessage::CheckAndImportAssignment(
						IndirectAssignmentCert {
							block_hash: head,
							validator: 0u32.into(),
							cert: garbage_assignment_cert(AssignmentCertKind::RelayVRFModulo {
								sample: 0,
							}),
						},
						0u32,
						tx,
					),
				},
			)
			.await;

			assert_eq!(rx.await, Ok(AssignmentCheckResult::Accepted));
		}
		virtual_overseer
	});
}
