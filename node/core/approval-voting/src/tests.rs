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
	AssignmentCert, AssignmentCertKind, VRFOutput, VRFProof,
	RELAY_VRF_MODULO_CONTEXT,
};

use parking_lot::Mutex;
use std::cell::RefCell;
use std::pin::Pin;
use std::sync::Arc;

const SLOT_DURATION_MILLIS: u64 = 5000;

#[derive(Default)]
struct MockClock {
	inner: Arc<Mutex<MockClockInner>>,
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

struct MockAssignmentCriteria;

impl AssignmentCriteria for MockAssignmentCriteria {
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
		Ok(0)
	}
}

#[derive(Default)]
pub struct TestStore {
	pub inner: RefCell<HashMap<Vec<u8>, Vec<u8>>>,
	// TODO: make sure these are consistent with inner
	pub block_entries: HashMap<Hash, BlockEntry>,
	pub candidate_entries: HashMap<CandidateHash, CandidateEntry>,
}

impl AuxStore for TestStore {
	fn insert_aux<'a, 'b: 'a, 'c: 'a, I, D>(&self, insertions: I, deletions: D) -> sp_blockchain::Result<()>
		where I: IntoIterator<Item = &'a (&'c [u8], &'c [u8])>, D: IntoIterator<Item = &'a &'b [u8]>
	{
		let mut store = self.inner.borrow_mut();

		// insertions before deletions.
		for (k, v) in insertions {
			store.insert(k.to_vec(), v.to_vec());
		}

		for k in deletions {
			store.remove(&k[..]);
		}

		Ok(())
	}

	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
		Ok(self.inner.borrow().get(key).map(|v| v.clone()))
	}
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
		assignment_criteria: Box::new(MockAssignmentCriteria),
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

#[test]
fn rejects_bad_assignment() {
	let mut state = single_session_state(1, SessionInfo::default());

	let assignment = IndirectAssignmentCert {
		block_hash: Hash::repeat_byte(0x01),
		validator: 0,
		cert: garbage_assignment_cert(
			AssignmentCertKind::RelayVRFModulo {
				sample: 0,
			},
		),
	};
	let candidate_index = 0;

	// TODO [now]: instantiate test store with block data.

	check_and_import_assignment(
		&mut state,
		assignment,
		candidate_index,
	).unwrap();

	// Check that the assignment's been imported.
}

#[test]
fn rejects_assignment_in_future() {

}

#[test]
fn rejects_assignment_with_unknown_candidate() {

}

#[test]
fn wakeup_scheduled_after_assignment_import() {

}

#[test]
fn loads_blocks_back_to_finality_or_stored() {

}

#[test]
fn rejects_approval_before_assignment() {

}

#[test]
fn full_approval_sets_flag_and_bit() {

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
