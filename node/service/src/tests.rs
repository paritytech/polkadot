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

use super::{relay_chain_selection::*, *};

use futures::channel::oneshot::Receiver;
use polkadot_node_primitives::approval::v2::VrfSignature;
use polkadot_node_subsystem::messages::{AllMessages, BlockDescription};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_test_client::Sr25519Keyring;
use sp_consensus_babe::{
	digests::{CompatibleDigestItem, PreDigest, SecondaryVRFPreDigest},
	VrfTranscript,
};
use sp_core::{crypto::VrfSecret, testing::TaskExecutor};
use sp_runtime::{testing::*, DigestItem};
use std::{
	collections::{BTreeMap, HashMap, HashSet},
	iter::IntoIterator,
};

use assert_matches::assert_matches;
use std::{sync::Arc, time::Duration};

use futures::{channel::oneshot, prelude::*};
use polkadot_node_subsystem::messages::{
	ApprovalVotingMessage, ChainSelectionMessage, DisputeCoordinatorMessage,
	HighestApprovedAncestorBlock,
};
use polkadot_primitives::{Block, BlockNumber, Hash, Header};

use polkadot_node_subsystem_test_helpers::TestSubsystemSender;
use polkadot_overseer::{SubsystemContext, SubsystemSender};

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ApprovalVotingMessage>;

#[async_trait::async_trait]
impl OverseerHandleT for TestSubsystemSender {
	async fn send_msg<M: Send + Into<AllMessages>>(&mut self, msg: M, _origin: &'static str) {
		TestSubsystemSender::send_message(self, msg.into()).await;
	}
}

struct TestHarness {
	virtual_overseer: VirtualOverseer,
	case_vars: CaseVars,
	/// The result of `fn finality_target` will be injected into the
	/// harness scope via this channel.
	finality_target_rx: Receiver<Option<Hash>>,
}

#[derive(Default)]
struct HarnessConfig;

fn test_harness<T: Future<Output = VirtualOverseer>>(
	case_vars: CaseVars,
	test: impl FnOnce(TestHarness) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter_level(log::LevelFilter::Trace)
		.try_init();

	let pool = TaskExecutor::new();
	let (mut context, virtual_overseer) = test_helpers::make_subsystem_context(pool);

	let (finality_target_tx, finality_target_rx) = oneshot::channel::<Option<Hash>>();

	let select_relay_chain = SelectRelayChainInner::<TestChainStorage, TestSubsystemSender>::new(
		Arc::new(case_vars.chain.clone()),
		context.sender().clone(),
		Default::default(),
		None,
	);

	let target_hash = case_vars.target_block;
	let selection_process = async move {
		let best = select_relay_chain
			.finality_target_with_longest_chain(target_hash, None)
			.await
			.unwrap();
		finality_target_tx.send(Some(best)).unwrap();
		()
	};

	let test_fut = test(TestHarness { virtual_overseer, case_vars, finality_target_rx });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(selection_process);
	futures::executor::block_on(future::join(
		async move {
			let _overseer = test_fut.await;
		},
		selection_process,
	));
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

	gum::trace!("Received message:\n{:?}", &msg);

	msg
}
async fn overseer_recv_with_timeout(
	overseer: &mut VirtualOverseer,
	timeout: Duration,
) -> Option<AllMessages> {
	gum::trace!("Waiting for message...");
	overseer.recv().timeout(timeout).await
}

const TIMEOUT: Duration = Duration::from_millis(2000);

// used for generating assignments where the validity of the VRF doesn't matter.
fn garbage_vrf_signature() -> VrfSignature {
	let transcript = VrfTranscript::new(b"test-garbage", &[]);
	Sr25519Keyring::Alice.pair().vrf_sign(&transcript.into())
}

/// Representation of a local representation
/// to extract information for finalization target
/// extraction.
#[derive(Debug, Default, Clone)]
struct TestChainStorage {
	blocks_by_hash: HashMap<Hash, Header>,
	blocks_at_height: BTreeMap<u32, Vec<Hash>>,
	disputed_blocks: HashSet<Hash>,
	approved_blocks: HashSet<Hash>,
	heads: HashSet<Hash>,
}

impl TestChainStorage {
	/// Fill the [`HighestApprovedAncestor`] structure with mostly
	/// correct data.
	pub fn highest_approved_ancestors(
		&self,
		minimum_block_number: BlockNumber,
		leaf: Hash,
	) -> Option<HighestApprovedAncestorBlock> {
		let mut descriptions = Vec::new();
		let mut block_hash = leaf;
		let mut highest_approved_ancestor = None;

		while let Some(block) = self.blocks_by_hash.get(&block_hash) {
			if minimum_block_number >= block.number {
				break
			}
			if !self.approved_blocks.contains(&block_hash) {
				highest_approved_ancestor = None;
				descriptions.clear();
			} else {
				if highest_approved_ancestor.is_none() {
					highest_approved_ancestor = Some((block_hash, block.number));
				}
				descriptions.push(BlockDescription {
					session: 1 as _, // dummy, not checked
					block_hash,
					candidates: vec![], // not relevant for any test cases
				});
			}
			block_hash = *block.parent_hash();
		}

		highest_approved_ancestor.map(|(hash, number)| HighestApprovedAncestorBlock {
			hash,
			number,
			descriptions: descriptions.into_iter().rev().collect(),
		})
	}

	/// Traverse backwards from leave down to block number.
	fn undisputed_chain(
		&self,
		base_blocknumber: BlockNumber,
		highest_approved_block_hash: Hash,
	) -> Option<Hash> {
		if self.disputed_blocks.is_empty() {
			return Some(highest_approved_block_hash)
		}

		let mut undisputed_chain = Some(highest_approved_block_hash);
		let mut block_hash = highest_approved_block_hash;
		while let Some(block) = self.blocks_by_hash.get(&block_hash) {
			let next = block.parent_hash();
			if self.disputed_blocks.contains(&block_hash) {
				undisputed_chain = Some(*next);
			}
			if block.number() == &base_blocknumber {
				break
			}
			block_hash = *next;
		}
		undisputed_chain
	}
}

impl HeaderProvider<Block> for TestChainStorage {
	fn header(&self, hash: Hash) -> sp_blockchain::Result<Option<Header>> {
		Ok(self.blocks_by_hash.get(&hash).cloned())
	}
	fn number(&self, hash: Hash) -> sp_blockchain::Result<Option<BlockNumber>> {
		self.header(hash).map(|opt| opt.map(|h| h.number))
	}
}

impl HeaderProviderProvider<Block> for TestChainStorage {
	type Provider = Self;
	fn header_provider(&self) -> &Self {
		self
	}
}

#[derive(Debug, Clone)]
struct ChainBuilder(pub TestChainStorage);

impl ChainBuilder {
	const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);
	const GENESIS_PARENT_HASH: Hash = Hash::repeat_byte(0x00);

	pub fn new() -> Self {
		let mut builder = Self(TestChainStorage::default());
		let _ = builder.add_block_inner(Self::GENESIS_HASH, Self::GENESIS_PARENT_HASH, 0);
		builder
	}

	pub fn add_block(&mut self, hash: Hash, parent_hash: Hash, number: u32) -> &mut Self {
		assert!(number != 0, "cannot add duplicate genesis block");
		assert!(hash != Self::GENESIS_HASH, "cannot add block with genesis hash");
		assert!(
			parent_hash != Self::GENESIS_PARENT_HASH,
			"cannot add block with genesis parent hash"
		);
		assert!(self.0.blocks_by_hash.len() < u8::MAX.into());
		self.add_block_inner(hash, parent_hash, number)
	}

	fn add_block_inner(&mut self, hash: Hash, parent_hash: Hash, number: u32) -> &mut Self {
		let header = ChainBuilder::make_header(parent_hash, number);
		assert!(
			self.0.blocks_by_hash.insert(hash, header).is_none(),
			"block with hash {:?} already exists",
			hash,
		);
		self.0.blocks_at_height.entry(number).or_insert_with(Vec::new).push(hash);
		self
	}

	pub fn fast_forward_approved(
		&mut self,
		branch_tag: u8,
		parent: Hash,
		block_number: BlockNumber,
	) -> Hash {
		let block = self.fast_forward(branch_tag, parent, block_number);
		let _ = self.0.approved_blocks.insert(block);
		block
	}

	/// Add a relay chain block that contains a disputed parachain block.
	/// For simplicity this is not modeled explicitly.
	pub fn fast_forward_disputed(
		&mut self,
		branch_tag: u8,
		parent: Hash,
		block_number: BlockNumber,
	) -> Hash {
		let block = self.fast_forward_approved(branch_tag, parent, block_number);
		let _ = self.0.disputed_blocks.insert(block);
		block
	}

	pub fn fast_forward(
		&mut self,
		branch_tag: u8,
		parent: Hash,
		block_number: BlockNumber,
	) -> Hash {
		let hash = Hash::repeat_byte((block_number as u8 | branch_tag) as u8);
		let _ = self.add_block(hash, parent, block_number);
		hash
	}

	pub fn set_heads(&mut self, heads: impl IntoIterator<Item = Hash>) {
		self.0.heads = heads.into_iter().collect();
	}

	pub fn init(self) -> TestChainStorage {
		self.0
	}

	fn make_header(parent_hash: Hash, number: u32) -> Header {
		let digest = {
			let mut digest = Digest::default();
			let vrf_signature = garbage_vrf_signature();
			digest.push(DigestItem::babe_pre_digest(PreDigest::SecondaryVRF(
				SecondaryVRFPreDigest {
					authority_index: 0,
					slot: 1.into(), // slot, unused
					vrf_signature,
				},
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
	chain: &TestChainStorage,
	virtual_overseer: &mut VirtualOverseer,
	target_block_hash: Hash,
	best_chain_containing_block: Option<Hash>,
	highest_approved_ancestor_block: Option<HighestApprovedAncestorBlock>,
	undisputed_chain: Option<Hash>,
) {
	let undisputed_chain = undisputed_chain.map(|x| (chain.number(x).unwrap().unwrap(), x));

	gum::trace!("best leaf response: {:?}", undisputed_chain);
	assert_matches!(
		overseer_recv(
			virtual_overseer
		).await,
		AllMessages::ChainSelection(ChainSelectionMessage::BestLeafContaining(
			target_hash,
			tx,
		))
		=> {
			assert_eq!(target_block_hash, target_hash, "TestIntegrity: target hashes always match. qed");
			tx.send(best_chain_containing_block).unwrap();
		}
	);

	if best_chain_containing_block.is_none() {
		return
	}

	gum::trace!("approved ancestor response: {:?}", undisputed_chain);
	assert_matches!(
		overseer_recv(
			virtual_overseer
		).await,
		AllMessages::ApprovalVoting(ApprovalVotingMessage::ApprovedAncestor(_block_hash, _block_number, tx))
		=> {
			tx.send(highest_approved_ancestor_block.clone()).unwrap();
		}
	);

	gum::trace!("determine undisputed chain response: {:?}", undisputed_chain);

	let target_block_number = chain.number(target_block_hash).unwrap().unwrap();
	assert_matches!(
		overseer_recv(
			virtual_overseer
		).await,
		AllMessages::DisputeCoordinator(
			DisputeCoordinatorMessage::DetermineUndisputedChain {
				base: _,
				block_descriptions: _,
				tx,
			}
		) => {
			tx.send(undisputed_chain.unwrap_or((target_block_number, target_block_hash))).unwrap();
	});
}

/// Straight forward test case, where the test is not
/// for integrity, but for different block relation structures.
fn run_specialized_test_w_harness<F: FnOnce() -> CaseVars>(case_var_provider: F) {
	test_harness(case_var_provider(), |test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
			finality_target_rx,
			case_vars:
				CaseVars {
					chain,
					target_block,
					best_chain_containing_block,
					highest_approved_ancestor_block,
					undisputed_chain,
					expected_finality_target_result,
				},
			..
		} = test_harness;

		// Verify test integrity: the provided highest approved
		// ancestor must match the chain derived one.
		let highest_approved_ancestor_w_desc = best_chain_containing_block
			.and_then(|best_chain_containing_block| {
				chain.blocks_by_hash.get(&target_block).map(|target_block_header| {
					let target_blocknumber = target_block_header.number;
					let highest_approved_ancestor_w_desc = chain.highest_approved_ancestors(
						target_blocknumber,
						best_chain_containing_block,
					);
					if let (
						Some(highest_approved_ancestor_w_desc),
						Some(highest_approved_ancestor_block),
					) = (&highest_approved_ancestor_w_desc, highest_approved_ancestor_block)
					{
						assert_eq!(
							highest_approved_ancestor_block, highest_approved_ancestor_w_desc.hash,
							"TestCaseIntegrity: Provided and expected approved ancestor hash mismatch: {:?} vs {:?}",
							highest_approved_ancestor_block, highest_approved_ancestor_w_desc.hash,
						);

						let expected = chain
							.undisputed_chain(target_blocknumber, highest_approved_ancestor_block);

						assert_eq!(
							expected, undisputed_chain,
							"TestCaseIntegrity: Provided and anticipated undisputed chain mismatch: {:?} vs {:?}",
							undisputed_chain, expected,
						);
					}
					highest_approved_ancestor_w_desc
				})
			})
			.flatten();

		test_skeleton(
			&chain,
			&mut virtual_overseer,
			target_block,
			best_chain_containing_block,
			highest_approved_ancestor_w_desc,
			undisputed_chain,
		)
		.await;

		assert_matches!(finality_target_rx.await,
		 	Ok(
		 		finality_target_val,
			) => assert_eq!(expected_finality_target_result, finality_target_val));

		virtual_overseer
	});
}

/// All variables relevant for a test case.
#[derive(Clone, Debug)]
struct CaseVars {
	/// Chain test _case_ definition.
	chain: TestChainStorage,

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

	/// The returned value by `fn finality_target`.
	expected_finality_target_result: Option<Hash>,
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3 --- 0xA4(!avail) --- 0xA5(!avail)
/// 			   \
/// 				`- 0xB2
/// ```
fn chain_undisputed() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_approved(0xA0, a1, 2);
	let a3 = builder.fast_forward_approved(0xA0, a2, 3);
	let a4 = builder.fast_forward(0xA0, a3, 4);
	let a5 = builder.fast_forward(0xA0, a4, 5);

	let b1 = builder.fast_forward_approved(0xB0, a1, 2);
	let b2 = builder.fast_forward_approved(0xB0, b1, 3);
	let b3 = builder.fast_forward_approved(0xB0, b2, 4);

	builder.set_heads(vec![a5, b3]);

	CaseVars {
		chain: builder.init(),
		target_block: a1,
		best_chain_containing_block: Some(a5),
		highest_approved_ancestor_block: Some(a3),
		undisputed_chain: Some(a3),
		expected_finality_target_result: Some(a3),
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3(disputed) --- 0xA4(!avail) --- 0xA5(!avail)
/// 			   \
/// 				`- 0xB2
/// ```
fn chain_0() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_approved(0xA0, a1, 2);
	let a3 = builder.fast_forward_disputed(0xA0, a2, 3);
	let a4 = builder.fast_forward(0xA0, a3, 4);
	let a5 = builder.fast_forward(0xA0, a4, 5);

	let b1 = builder.fast_forward_approved(0xB0, a1, 2);
	let b2 = builder.fast_forward_approved(0xB0, b1, 3);
	let b3 = builder.fast_forward_approved(0xB0, b2, 4);

	builder.set_heads(vec![a5, b3]);

	CaseVars {
		chain: builder.init(),
		target_block: a1,
		best_chain_containing_block: Some(a5),
		highest_approved_ancestor_block: Some(a3),
		undisputed_chain: Some(a2),
		expected_finality_target_result: Some(a2),
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2(disputed) --- 0xA3
/// 			   \
/// 				`- 0xB2 --- 0xB3(!available)
/// ```
fn chain_1() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_disputed(0xA0, a1, 2);
	let a3 = builder.fast_forward_approved(0xA0, a2, 3);

	let b2 = builder.fast_forward_approved(0xB0, a1, 2);
	let b3 = builder.fast_forward(0xB0, b2, 3);

	builder.set_heads(vec![a3, b3]);

	CaseVars {
		chain: builder.init(),
		target_block: a1,
		best_chain_containing_block: Some(b3),
		highest_approved_ancestor_block: Some(b2),
		undisputed_chain: Some(b2),
		expected_finality_target_result: Some(b2),
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2(disputed) --- 0xA3
/// 			   \
/// 				`- 0xB2 --- 0xB3
/// ```
fn chain_2() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_disputed(0xA0, a1, 2);
	let a3 = builder.fast_forward_approved(0xA0, a2, 3);

	let b2 = builder.fast_forward_approved(0xB0, a1, 2);
	let b3 = builder.fast_forward_approved(0xB0, b2, 3);

	builder.set_heads(vec![a3, b3]);

	CaseVars {
		chain: builder.init(),
		target_block: a3,
		best_chain_containing_block: Some(a3),
		highest_approved_ancestor_block: Some(a3),
		undisputed_chain: Some(a1),
		expected_finality_target_result: Some(a1),
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3(disputed)
/// 			   \
/// 				`- 0xB2 --- 0xB3
/// ```
fn chain_3() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_approved(0xA0, a1, 2);
	let a3 = builder.fast_forward_disputed(0xA0, a2, 3);

	let b2 = builder.fast_forward_approved(0xB0, a1, 2);
	let b3 = builder.fast_forward_approved(0xB0, b2, 3);

	builder.set_heads(vec![a3, b3]);

	CaseVars {
		chain: builder.init(),
		target_block: a2,
		best_chain_containing_block: Some(a3),
		highest_approved_ancestor_block: Some(a3),
		undisputed_chain: Some(a2),
		expected_finality_target_result: Some(a2),
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3(disputed)
/// 			   \
/// 				`- 0xB2 --- 0xB3
///
/// 	            ? --- NEX(does_not_exist)
/// ```
fn chain_4() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_approved(0xA0, a1, 2);
	let a3 = builder.fast_forward_disputed(0xA0, a2, 3);

	let b2 = builder.fast_forward_approved(0xB0, a1, 2);
	let b3 = builder.fast_forward_approved(0xB0, b2, 3);

	builder.set_heads(vec![a3, b3]);

	let does_not_exist = Hash::repeat_byte(0xCC);
	CaseVars {
		chain: builder.init(),
		target_block: does_not_exist,
		best_chain_containing_block: None,
		highest_approved_ancestor_block: None,
		undisputed_chain: None,
		expected_finality_target_result: Some(does_not_exist),
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2
/// ```
fn chain_5() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.fast_forward_approved(0xA0, head, 1);
	let a2 = builder.fast_forward_approved(0xA0, a1, 2);

	builder.set_heads(vec![a2]);

	CaseVars {
		chain: builder.init(),
		target_block: a2,
		best_chain_containing_block: Some(a2),
		highest_approved_ancestor_block: Some(a2),
		undisputed_chain: Some(a2),
		expected_finality_target_result: Some(a2),
	}
}

/// ```raw
/// genesis -- 0xB2 -- 0xD2 -- .. -- 0xD8 -- 0xC8(unapproved) -- .. -- 0xCF(unapproved)
/// ```
fn chain_6() -> CaseVars {
	let head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let b1 = builder.fast_forward_approved(0xB0, head, 1);

	let mut previous = b1;
	let mut approved = b1;
	for block_number in 2_u32..16 {
		if block_number <= 8 {
			previous = builder.fast_forward_approved(0xD0, previous, block_number as _);
			approved = previous;
		} else {
			previous = builder.fast_forward(0xA0, previous, block_number as _);
		}
	}
	let leaf = previous;

	builder.set_heads(vec![leaf]);

	let chain = builder.init();

	gum::trace!(highest_approved = ?chain.highest_approved_ancestors(1, leaf));
	gum::trace!(undisputed = ?chain.undisputed_chain(1, approved));
	CaseVars {
		chain,
		target_block: b1,
		best_chain_containing_block: Some(leaf),
		highest_approved_ancestor_block: Some(approved),
		undisputed_chain: Some(approved),
		expected_finality_target_result: Some(approved),
	}
}

#[test]
fn chain_sel_undisputed() {
	run_specialized_test_w_harness(chain_undisputed);
}

#[test]
fn chain_sel_0() {
	run_specialized_test_w_harness(chain_0);
}

#[test]
fn chain_sel_1() {
	run_specialized_test_w_harness(chain_1);
}

#[test]
fn chain_sel_2() {
	run_specialized_test_w_harness(chain_2);
}

#[test]
fn chain_sel_3() {
	run_specialized_test_w_harness(chain_3);
}

#[test]
fn chain_sel_4_target_hash_value_not_contained() {
	run_specialized_test_w_harness(chain_4);
}

#[test]
fn chain_sel_5_best_is_target_hash() {
	run_specialized_test_w_harness(chain_5);
}

#[test]
fn chain_sel_6_approval_lag() {
	run_specialized_test_w_harness(chain_6);
}
