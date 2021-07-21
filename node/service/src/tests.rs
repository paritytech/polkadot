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
use sp_runtime::DigestItem;
use std::time::Duration;
use futures::channel::oneshot::Receiver;
use polkadot_overseer::HeadSupportsParachains;
use polkadot_primitives::v1::{
	CoreIndex, GroupIndex, ValidatorSignature, Header, CandidateEvent,
};
use polkadot_primitives::v1::Slot;
use polkadot_node_subsystem::{ActivatedLeaf, ActiveLeavesUpdate, LeafStatus};
use polkadot_node_primitives::approval::{
	AssignmentCert, AssignmentCertKind, VRFOutput, VRFProof,
	RELAY_VRF_MODULO_CONTEXT, DelayTranche,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem::messages::{AllMessages, ApprovalVotingMessage, AssignmentCheckResult};
use polkadot_node_subsystem_util::TimeoutExt;
use consensus_common::{Error as ConsensusError, SelectChain};
use polkadot_subsystem::messages::BlockDescription;
use polkadot_subsystem::*;

use sp_runtime::testing::*;

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use sp_keystore::CryptoStore;
use assert_matches::assert_matches;

use super::relay_chain_selection::*;
use {
	polkadot_primitives::v1::{
		Hash, BlockNumber, Block as PolkadotBlock, Header as PolkadotHeader,
	},
	polkadot_subsystem::messages::{ApprovalVotingMessage, HighestApprovedAncestorBlock, ChainSelectionMessage, DisputeCoordinatorMessage},
	polkadot_node_subsystem_util::metrics::{self, prometheus},
	polkadot_overseer::Handle,
	futures::channel::oneshot,
	consensus_common::{Error as ConsensusError, SelectChain},
	sp_blockchain::HeaderBackend,
	sp_runtime::generic::BlockId,
	std::sync::Arc,
};

const SLOT_DURATION_MILLIS: u64 = 100;

#[cfg(test)]
pub mod test_constants {
	use crate::approval_db::v1::Config as DatabaseConfig;
	const DATA_COL: u32 = 0;
	pub(crate) const NUM_COLUMNS: u32 = 1;

	pub(crate) const TEST_CONFIG: DatabaseConfig = DatabaseConfig {
		col_data: DATA_COL,
	};
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


type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ApprovalVotingMessage>;

struct TestHarness {
	virtual_overseer: VirtualOverseer,
	/// The result of `fn finality_target` will be injected into the
	/// harness scope via this channel.
	finality_target_rx: Receiver<Option<(BlockNumber, Hash)>>,
}

#[derive(Default)]
struct HarnessConfig;

fn test_harness<T: Future<Output = VirtualOverseer>>(
	config: HarnessConfig,
	case_vars: CaseVars,
	test: impl FnOnce(TestHarness) -> T,
) -> Option<(BlockNumber, Block)> {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool);


	let (finality_target_tx, finality_target_rx) = oneshot::channel();

	let keystore = futures::executor::block_on(
		make_keystore(&[Sr25519Keyring::Alice])
	);

	let store = Arc::new(TestStore::default());
	let mut select_relay_chain = SelectRelayChain::new(store.clone(), virtual_overseer, Default::default());

	let HarnessConfig {
	    block_hash,
		block_number,
	} = config;

	select_relay_chain.connect_overseer_handler(handle);

	let selection_process =
		async move {
			let best = select_relay_chain.finality_target(hash, block_number).await;
			finality_target_tx.send(best).unwrap();
		};

	let test_fut = test(TestHarness {
		virtual_overseer,
		finality_target_rx,
	});

	futures::pin_mut!(test_fut);
	futures::pin_mut!(selection_process);

	futures::executor::block_on(
		future::join(
			async move {
				let mut overseer = test_fut.await;
				overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
			},
			selection_process
		)
	).1.unwrap()
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




// sets up a keystore with the given keyring accounts.
async fn make_keystore(accounts: &[Sr25519Keyring]) -> LocalKeystore {
	let store = LocalKeystore::in_memory();

	for s in accounts.iter().copied().map(|k| k.to_seed()) {
		store.sr25519_generate_new(
			ASSIGNMENT_KEY_TYPE_ID,
			Some(s.as_str()),
		).await.unwrap();
	}

	store
}


// used for generating assignments where the validity of the VRF doesn't matter.
fn garbage_vrf() -> (VRFOutput, VRFProof) {
	let key = Sr25519Keyring::Alice.pair();
	let key: &schnorrkel::Keypair = key.as_ref();

	let (o, p, _) = key.vrf_sign(Transcript::new(b"test-garbage"));
	(VRFOutput(o.to_output()), VRFProof(p))
}


// nice to have
const A1: Hash = Hash::repeat_byte(0xA1);
const A2: Hash = Hash::repeat_byte(0xA2);
const A3: Hash = Hash::repeat_byte(0xA3);
const A4: Hash = Hash::repeat_byte(0xA4);

const B1: Hash = Hash::repeat_byte(0xB1);
const B2: Hash = Hash::repeat_byte(0xB2);
const B3: Hash = Hash::repeat_byte(0xB3);
const B4: Hash = Hash::repeat_byte(0xB4);



/// Representation of a local representation
/// to extract information for finalization target
/// extraction.
#[derive(Debug, Default, Clone)]
struct ChainContent {
	blocks_by_hash: HashMap<Hash, Header>,
	blocks_at_height: BTreeMap<u32, Vec<Hash>>,
	disputed_blocks: HashSet<Hash>,
	available_blocks: HashSet<Hash>,

	heads: HashSet<Hash>,
}


#[derive(Debug, Clone)]
struct ChainBuilder(pub ChainContent);

impl ChainBuilder {
	const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);
	const GENESIS_PARENT_HASH: Hash = Hash::repeat_byte(0x00);

	pub fn new() -> Self {
		let mut builder = Self(
			ChainContent::default()
		);
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
		assert!(parent_hash != Self::GENESIS_PARENT_HASH, "cannot add block with genesis parent hash");
		assert!(self.0.blocks_by_hash.len() < u8::MAX.into());
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
			self.0.blocks_by_hash.insert(hash, header).is_none(),
			"block with hash {:?} already exists", hash,
		);
		self.0.blocks_at_height.entry(number).or_insert_with(Vec::new).push(hash);
		self
	}

	pub fn ff_available(&mut self, branch_tag: u8, parent: Hash, block_number: BlockNumber, slot: impl Into<Slot>) -> Hash {
		let block = self.ff(builder, branch_tag, parent, block_number, slot);
		let _ = self.0.available_blocks.insert(block);
		self.update_best(block);
		block
	}

	/// Add a relay chain block that contains a disputed parachain block.
	/// For simplicity this is not modeled explicitly.
	pub fn ff_disputed(&mut self, branch_tag: u8, parent: Hash, block_number: BlockNumber, slot: impl Into<Slot>) -> Hash {
		let block = self.ff_available(builder, branch_tag, parent, block_number, slot);
		let _ = self.0.disputed_blocks.insert(block);
		self.update_best(block);
		block
	}

	pub fn ff(&mut self, branch_tag: u8, parent: Hash, block_number: BlockNumber, slot: impl Into<Slot>) -> Hash {
		let slot: Slot = i.into();
		let hash = Hash::repeat_byte((i & 0xA0) as u8);
		self.add_block(hash, head, slot, i);
		hash
	}

	pub fn set_heads(&mut self, heads: impl IntoIter<Hash>) {
		self.0.heads = heads.into_iter().collect();
	}

	pub fn init(mut self) -> ChainContent {
		self.0
	}

	/// Fill the `HighestApprovedAncestor` structure with mostly
	/// correct data.
	pub fn highest_approved_ancestors(&self, target_block: Hash) -> Option<HighestApprovedAncestorBlock> {
		let hash = target_block;
		let number = self.0.blocks_by_hash.get(&target_block)?.number;

		let header = self.0.blocks_by_hash.get(&target_block).unwrap();

		let mut descriptions = Vec::new();
		let mut block = target_block;
		let mut highest_approved_ancestor = None;

		while let Some(block) = self.0.blocks_by_hash.get(&block) {
			descriptions.push(BlockDescription {
				session: 1 as _, // dummy
				block_hash: next,
				candidates: vec![], // not relevant for all test cases
			});
			if self.0.available_blocks.contains(&block) {
				highest_approved_ancestor = Some(block);
				break;
			}
			if let Some(next) = block.parent {
				block = next
			} else {
				break;
			}
		}

		if highest_approved_ancestor.is_none() {
			return None
		}

		Some(HighestApprovedAncestorBlock {
			hash,
			number,
			descriptions: descriptions.into_iter().rev().collect(),
		})
	}

	fn make_header(
		parent_hash: Hash,
		slot: Slot,
		number: u32,
	) -> Header {
		let digest = {
			let mut digest = Digest::default();
			let (vrf_output, vrf_proof) = garbage_vrf();
			digest.push(DigestItem::babe_pre_digest(PreDigest::SecondaryVRF(
				SecondaryVRFPreDigest {
					authority_index: 0,
					slot,
					vrf_output,
					vrf_proof,
				}
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

/// Generalized sequence of the test, based on
/// the messages being sent out by the `fn finality_target`
/// Depends on a particular `target_hash`
/// that is passed to `finality_target` block number.
async fn test_skeleton(
	best_chain_containing_block: Option<Hash>,
	highest_approved_ancestor_block: Option<HighestApprovedAncestorBlock>,
	undisputed_chain: Option<(BlockNumber, Hash)>,
) {
	assert_matches!(
		overseer_recv(
			&mut virtual_overseer
		).await,
		AllMessages::ChainSelection(ChainSelectionMessage::BestLeafContaining(target_hash, tx))
		=> {
			tx.send(best_chain_containing_block);
		}
	);

	assert_matches!(
		overseer_recv(
			&mut virtual_overseer
		).await,
		AllMessages::ApprovalVoting(ApprovalVotingMessage::ApprovedAncestor(block_hash, block_number, tx))
		=> {
			tx.send(highest_approved_ancestor_block);
		}
	);

	assert_matches!(
		overseer_recv(
			&mut virtual_overseer
		).await,
		AllMessages::DisputeCoordinator(
			DisputeCoordinatorMessage::DetermineUndisputedChain {
				base_number,
				block_descriptions,
				tx,
			}
		) => {
			tx.send(undisputed_chain);
	});

}


/// Straight forward test case, where the test is not
/// for integrity, but for different block relation structures.
fn run_specialized_test_w_harness<T>()
where
	T: FnOnce() -> CaseVars,
{
	test_harness(Default::default(), chain_0(), |test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
			finality_target_rx,
			case_vars: CaseVars {
				heads,
				chain,
				target_block,
				best_chain_containing_block,
				highest_approved_ancestor_block,
				..
			},
			..
		} = test_harness;

		let highest_approved_ancestor_w_desc = chain.highest_approved_ancestor(target_hash);

		if let (Some(highest_approved_ancestor_w_desc), Some(highest_approved_ancestor_block)) = (highest_approved_ancestor_w_desc, highest_approved_ancestor_block) {
			assert_eq!(
				highest_approved_ancestor_block,
				highest_approved_ancestor_w_desc.hash,
				"Derived block hash and provided do not match {:?} vs {:?}",
				highest_approved_ancestor_block,
				highest_approved_ancestor_w_desc.hash,
			);
		}

		test_skeleton(
			best_chain_containing_block,
			highest_approved_ancestor_w_desc,
			undisputed_chain
		).await;

		assert_matches!(finality_target_rx.await,
			Some(
				val,
			) => assert_eq!(undisputed_chain, val));

		virtual_overseer
	});
}


/// All variables relevant for a test case.
struct CaseVars {
	/// Abstract chain definition.
	// TODO must be tied to the `TestStore` content.
	chain: ChainContent,

	/// All heads of the chain.
	heads: Vec<Hash>,

	/// The target block to be finalized.
	target_block: Hash,

	/// Response to the `target_block` request, must be a chain-head.
	/// `None` if no such chain exists.
	best_chain_containing_block: Option<Hash>,

	/// Resulting best estimate, before considering
	/// the disputed state of blocks.
	highest_approved_ancestor_block: Option<Hash>,

	/// Equal to the previous, unless there are disputes.
	/// The backtracked version of this must _never_
	/// contain a disputed block.
	undisputed_chain: Option<Hash>,
}


/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3 --- 0xA4(!avail) --- 0xA5(!avail)
///               \
///                `- 0xB2
/// ```
fn chain_0() -> CaseVars {
	let mut head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.ff_available(&mut builder, 0xA0, head, 1, 1 );
	let a2 = builder.ff_available(&mut builder, 0xA0, a1, 2, 1 );
	let a3 = builder.ff_available(&mut builder, 0xA0, a2, 3, 1 );
	let a4 = builder.ff(&mut builder, 0xA0, a3, 4, 1 );
	let a5 = builder.ff(&mut builder, 0xA0, a4, 5, 1 );
	let b1 = builder.ff_available(&mut builder, 0xB0, a1, 2, 2 );
	let b2 = builder.ff_available(&mut builder, 0xB0, b1, 2, 2 );
	let b3 = builder.ff_available(&mut builder, 0xB0, b2, 2, 2 );

	CaseVars {
		chain: builder.init(),
		heads: vec![a3, b2],
		target_block: A4,
		best_chain_containing_block: Some(A5),
		highest_approved_ancestor_block: Some(A3),
		undisputed_chain: Some(A3),
	}
}


/// ```raw
/// genesis -- 0xA1 --- 0xA2(disputed) --- 0xA3
///               \
///                `- 0xB2 --- 0xB3(!available)
/// ```
fn chain_1() -> CaseVars {
	let mut head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.ff_available(&mut builder, 0xA0, head, 1, 1 );
	let a2 = builder.ff_disputed(&mut builder, 0xA0, a1, 2, 1 );
	let a3 = builder.ff_available(&mut builder, 0xA0, a2, 3, 1 );
	let b2 = builder.ff_available(&mut builder, 0xB0, a1, 2, 2 );
	let b3 = builder.ff_available(&mut builder, 0xB0, a1, 3, 2 );

	CaseVars {
		chain: builder.init(),
		heads: vec![a3, b3],
		target_block: A1,
		best_chain_containing_block: Some(B3),
		highest_approved_ancestor_block: Some(B2),
		undisputed_chain: Some(B2),
	}
}


/// ```raw
/// genesis -- 0xA1 --- 0xA2(disputed) --- 0xA3
///               \
///                `- 0xB2 --- 0xB3
/// ```
fn chain_2() -> CaseVars {
	let mut head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.ff_available(&mut builder, 0xA0, head, 1, 1 );
	let a2 = builder.ff_disputed(&mut builder, 0xA0, a1, 2, 1 );
	let a3 = builder.ff_available(&mut builder, 0xA0, a2, 3, 1 );
	let b2 = builder.ff_available(&mut builder, 0xB0, a1, 2, 2 );
	let b3 = builder.ff_available(&mut builder, 0xB0, a1, 3, 2 );

	CaseVars {
		chain: builder.init(),
		heads: vec![a3, b3],
		target_block: ChainBuilder::A3,
		best_chain_containing_block: Some(ChainBuilder::A3),
		highest_approved_ancestor_block: Some(ChainBuilder::A3),
		undisputed_chain: None,
	}
}



/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3(disputed)
///               \
///                `- 0xB2 --- 0xB3
/// ```
fn chain_3() -> Vec<Hash> {
	let mut head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.ff_available(&mut builder, 0xA0, head, 1, 1 );
	let a2 = builder.ff_available(&mut builder, 0xA0, a1, 2, 1 );
	let a3 = builder.ff_disputed(&mut builder, 0xA0, a2, 3, 1 );
	let b2 = builder.ff_available(&mut builder, 0xB0, a1, 2, 2 );
	let b3 = builder.ff_available(&mut builder, 0xB0, a1, 3, 2 );

	CaseVars{
		chain: builder.init(),
		heads: vec![a3, b3],
		target_block: A2,
		best_chain_containing_block: Some(A3),
		highest_approved_ancestor_block: Some(A3),
		undisputed_chain: Some(A2),
	}
}



#[test]
fn chain_sel_0() {
	run_specialized_test_w_harness::<chain_0>();
}


#[test]
fn chain_sel_1() {
	run_specialized_test_w_harness::<chain_1>();
}

#[test]
fn chain_sel_2() {
	run_specialized_test_w_harness::<chain_2>();
}


#[test]
fn chain_sel_4_target_hash_value_not_contained() {
	todo!("")
}


#[test]
fn chain_sel_5_best_is_target_hash() {
	todo!("")
}


#[test]
fn chain_sel_5_huge_lag() {
	todo!("")
}



#[test]
fn chain_sel_6_invalid_ordering() {
	todo!("")
}
