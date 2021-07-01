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
use std::time::Duration;
use polkadot_overseer::HeadSupportsParachains;
use polkadot_primitives::v1::{
	CoreIndex, GroupIndex, ValidatorSignature, Header, CandidateEvent,
};
use polkadot_node_subsystem::{ActivatedLeaf, ActiveLeavesUpdate, LeafStatus};
use polkadot_node_primitives::approval::{
	AssignmentCert, AssignmentCertKind, VRFOutput, VRFProof,
	RELAY_VRF_MODULO_CONTEXT, DelayTranche,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem::messages::{AllMessages, ApprovalVotingMessage, AssignmentCheckResult};
use polkadot_node_subsystem_util::TimeoutExt;

use parking_lot::Mutex;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use sp_keyring::sr25519::Keyring as Sr25519Keyring;
use sp_keystore::CryptoStore;
use assert_matches::assert_matches;

use crate::ops::StoredBlockRange;
use crate::backend::BackendWriteOp;
use super::import::tests::{
	BabeEpoch, BabeEpochConfiguration, AllowedSlots, Digest, garbage_vrf, DigestItem, PreDigest,
	SecondaryVRFPreDigest, CompatibleDigestItem,
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
		TestSyncOracle {
			flag: flag.clone(),
			done_syncing_sender: Arc::new(Mutex::new(Some(tx))),
		},
		TestSyncOracleHandle {
			flag,
			done_syncing_receiver: rx,
		}
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

	pub(crate) const TEST_CONFIG: DatabaseConfig = DatabaseConfig {
		col_data: DATA_COL,
	};
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
		let drain_up_to = self.wakeups.binary_search_by_key(
			&(up_to + 1),
			|w| w.0,
		).unwrap_or_else(|i| i);

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
		_leaving_cores: Vec<(CandidateHash, polkadot_primitives::v1::CoreIndex, polkadot_primitives::v1::GroupIndex)>,
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
	stored_block_range: Option<StoredBlockRange>,
	blocks_at_height: HashMap<BlockNumber, Vec<Hash>>,
	block_entries: HashMap<Hash, BlockEntry>,
	candidate_entries: HashMap<CandidateHash, CandidateEntry>,
}

impl Backend for TestStore {
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

	fn load_blocks_at_height(
		&self,
		height: &BlockNumber,
	) -> SubsystemResult<Vec<Hash>> {
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
		where I: IntoIterator<Item = BackendWriteOp>
	{
		for op in ops {
			match op {
				BackendWriteOp::WriteStoredBlockRange(stored_block_range) => {
					self.stored_block_range = Some(stored_block_range);
				}
				BackendWriteOp::WriteBlocksAtHeight(h, blocks) => {
					self.blocks_at_height.insert(h, blocks);
				}
				BackendWriteOp::DeleteBlocksAtHeight(h) => {
					let _ = self.blocks_at_height.remove(&h);
				}
				BackendWriteOp::WriteBlockEntry(block_entry) => {
					self.block_entries.insert(block_entry.block_hash(), block_entry);
				}
				BackendWriteOp::DeleteBlockEntry(hash) => {
					let _ = self.block_entries.remove(&hash);
				}
				BackendWriteOp::WriteCandidateEntry(candidate_entry) => {
					self.candidate_entries.insert(candidate_entry.candidate_receipt().hash(), candidate_entry);
				}
				BackendWriteOp::DeleteCandidateEntry(candidate_hash) => {
					let _ = self.candidate_entries.remove(&candidate_hash);
				}
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

	let HarnessConfig {
		tick_start,
		assigned_tranche,
	} = config;

	let clock = Box::new(MockClock::new(tick_start));
	let subsystem = run(
		context,
		ApprovalVotingSubsystem::with_config(
			Config{
				col_data: test_constants::TEST_CONFIG.col_data,
				 slot_duration_millis: 100u64,
			},
			Arc::new(kvdb_memorydb::create(test_constants::NUM_COLUMNS)),
			Arc::new(keystore),
			sync_oracle,
			Metrics::default(),
		),
		clock.clone(),
		Box::new(MockAssignmentCriteria::check_only(move || { Ok(assigned_tranche) })),
		store,
	);

	let test_fut = test(TestHarness {
		virtual_overseer,
		clock,
	});

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	futures::executor::block_on(future::join(async move {
		let mut overseer = test_fut.await;
		overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
	}, subsystem)).1.unwrap();
}

async fn overseer_send(
	overseer: &mut VirtualOverseer,
	msg: FromOverseer<ApprovalVotingMessage>,
) {
	tracing::trace!("Sending message:\n{:?}", &msg);
	overseer
		.send(msg)
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(
	overseer: &mut VirtualOverseer,
) -> AllMessages {
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
	overseer
		.recv()
		.timeout(timeout)
		.await
}

const TIMEOUT: Duration = Duration::from_millis(2000);
async fn overseer_signal(
	overseer: &mut VirtualOverseer,
	signal: OverseerSignal,
) {
	overseer
		.send(FromOverseer::Signal(signal))
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
}
