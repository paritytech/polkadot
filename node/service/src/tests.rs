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
use super::relay_chain_selection::*;

use futures::channel::oneshot::Receiver;
use sp_consensus_babe::Transcript;
use sp_consensus_babe::digests::CompatibleDigestItem;
use polkadot_primitives::v1::ASSIGNMENT_KEY_TYPE_ID;
use polkadot_subsystem::messages::AllMessages;
use polkadot_test_client::Sr25519Keyring;
use polkadot_node_primitives::approval::{
	VRFOutput, VRFProof,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v1::Slot;
use polkadot_subsystem::messages::BlockDescription;
use polkadot_subsystem::*;
use sp_runtime::{
	DigestItem,
};
use sp_runtime::testing::*;
use sp_consensus_babe::digests::PreDigest;
use sp_consensus_babe::digests::SecondaryVRFPreDigest;
use std::collections::{HashMap, HashSet, BTreeMap};
use std::iter::IntoIterator;

use assert_matches::assert_matches;
use sp_keystore::CryptoStore;
use std::sync::Arc;
use std::time::Duration;

use consensus_common::SelectChain;
use polkadot_primitives::v1::{Block, BlockNumber, Hash, Header};
use polkadot_subsystem::messages::{
	ApprovalVotingMessage, ChainSelectionMessage, DisputeCoordinatorMessage, HighestApprovedAncestorBlock,
};

use sp_blockchain::HeaderBackend;
use sc_keystore::LocalKeystore;
use futures::prelude::*;
use futures::channel::oneshot;

use crate::relay_chain_selection::{HeaderProvider, HeaderProviderProvider};
use polkadot_node_subsystem_test_helpers::TestSubsystemSender;
use polkadot_overseer::{SubsystemSender, SubsystemContext};

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
	let pool = sp_core::testing::TaskExecutor::new();
	let (mut context, virtual_overseer) = test_helpers::make_subsystem_context(pool);

	let (finality_target_tx, finality_target_rx) = oneshot::channel::<Option<Hash>>();

	// let keystore = futures::executor::block_on(make_keystore(&[Sr25519Keyring::Alice]));

	let mut select_relay_chain =
		SelectRelayChain::<
			ChainContent,
			TestSubsystemSender,
		>::new(
		Arc::new(case_vars.chain.clone()), context.sender().clone(), Default::default()
		);

	let target_hash = case_vars.target_block.clone();
	let selection_process = async move {
		let best = select_relay_chain.finality_target(target_hash, None).await.unwrap();
		finality_target_tx.send(best).unwrap();
		()
	};

	let test_fut = test(TestHarness { virtual_overseer, case_vars, finality_target_rx });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(selection_process);

	let x = futures::executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		},
		selection_process,
	))
	.1;
}

async fn overseer_send(overseer: &mut VirtualOverseer, msg: FromOverseer<ApprovalVotingMessage>) {
	tracing::trace!("Sending message:\n{:?}", &msg);
	overseer.send(msg).timeout(TIMEOUT).await.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
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

// sets up a keystore with the given keyring accounts.
// async fn make_keystore(accounts: &[Sr25519Keyring]) -> LocalKeystore {
// 	let store = LocalKeystore::in_memory();

// 	for s in accounts.iter().copied().map(|k| k.to_seed()) {
// 		store.sr25519_generate_new(ASSIGNMENT_KEY_TYPE_ID, Some(s.as_str())).await.unwrap();
// 	}

// 	store
// }

// used for generating assignments where the validity of the VRF doesn't matter.
fn garbage_vrf() -> (VRFOutput, VRFProof) {
	let key = Sr25519Keyring::Alice.pair();
	let key = key.as_ref();

	let (o, p, _) = key.vrf_sign(Transcript::new(b"test-garbage"));
	(VRFOutput(o.to_output()), VRFProof(p))
}

// nice to have
const A1: Hash = Hash::repeat_byte(0xA1);
const A2: Hash = Hash::repeat_byte(0xA2);
const A3: Hash = Hash::repeat_byte(0xA3);
const A4: Hash = Hash::repeat_byte(0xA4);
const A5: Hash = Hash::repeat_byte(0xA5);

const B1: Hash = Hash::repeat_byte(0xB1);
const B2: Hash = Hash::repeat_byte(0xB2);
const B3: Hash = Hash::repeat_byte(0xB3);

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

impl ChainContent {

	/// Fill the `HighestApprovedAncestor` structure with mostly
	/// correct data.
	pub fn highest_approved_ancestors(&self, target_block: Hash) -> Option<HighestApprovedAncestorBlock> {
		let hash = target_block;
		let number = self.blocks_by_hash.get(&target_block)?.number;

		let header = self.blocks_by_hash.get(&target_block).unwrap();

		let mut descriptions = Vec::new();
		let mut block_hash = target_block;
		let mut highest_approved_ancestor = None;

		while let Some(block) = self.blocks_by_hash.get(&block_hash) {
			block_hash = block.hash();
			descriptions.push(BlockDescription {
				session: 1 as _, // dummy
				block_hash,
				candidates: vec![], // not relevant for all test cases
			});
			if self.available_blocks.contains(&block_hash) {
				highest_approved_ancestor = Some(block_hash);
				break;
			}
			let next = block.parent_hash();
			if &target_block != block.parent_hash() {
				block_hash = *next;
			} else {
				break;
			}
		}

		if highest_approved_ancestor.is_none() {
			return None;
		}

		Some(HighestApprovedAncestorBlock { hash, number, descriptions: descriptions.into_iter().rev().collect() })
	}
}


impl HeaderProvider<Block> for ChainContent {
	fn header(&self, hash: Hash) -> sp_blockchain::Result<Option<Header>> {
		Ok(self.blocks_by_hash.get(&hash).cloned())
	}
	fn number(&self, hash: Hash) -> sp_blockchain::Result<Option<BlockNumber>> {
		self.header(hash).map(|opt| {
			opt.map(|h| h.number)
		})
	}
}

impl HeaderProviderProvider<Block> for ChainContent {
	type Provider = Self;
	fn header_provider(&self) -> &Self {
		self
	}
}


#[derive(Debug, Clone)]
struct ChainBuilder(pub ChainContent);

impl ChainBuilder {
	const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);
	const GENESIS_PARENT_HASH: Hash = Hash::repeat_byte(0x00);

	pub fn new() -> Self {
		let mut builder = Self(ChainContent::default());
		let _ = builder.add_block_inner(Self::GENESIS_HASH, Self::GENESIS_PARENT_HASH, Slot::from(0), 0);
		builder
	}

	pub fn add_block<'a>(&'a mut self, hash: Hash, parent_hash: Hash, slot: Slot, number: u32) -> &'a mut Self {
		assert!(number != 0, "cannot add duplicate genesis block");
		assert!(hash != Self::GENESIS_HASH, "cannot add block with genesis hash");
		assert!(parent_hash != Self::GENESIS_PARENT_HASH, "cannot add block with genesis parent hash");
		assert!(self.0.blocks_by_hash.len() < u8::MAX.into());
		self.add_block_inner(hash, parent_hash, slot, number)
	}

	fn add_block_inner<'a>(&'a mut self, hash: Hash, parent_hash: Hash, slot: Slot, number: u32) -> &'a mut Self {
		let header = ChainBuilder::make_header(parent_hash, slot, number);
		assert!(self.0.blocks_by_hash.insert(hash, header).is_none(), "block with hash {:?} already exists", hash,);
		self.0.blocks_at_height.entry(number).or_insert_with(Vec::new).push(hash);
		self
	}

	pub fn ff_available(
		&mut self,
		branch_tag: u8,
		parent: Hash,
		block_number: BlockNumber,
		slot: impl Into<Slot>,
	) -> Hash {
		let block = self.ff(branch_tag, parent, block_number, slot);
		let _ = self.0.available_blocks.insert(block);
		block
	}

	/// Add a relay chain block that contains a disputed parachain block.
	/// For simplicity this is not modeled explicitly.
	pub fn ff_disputed(
		&mut self,
		branch_tag: u8,
		parent: Hash,
		block_number: BlockNumber,
		slot: impl Into<Slot>,
	) -> Hash {
		let block = self.ff_available(branch_tag, parent, block_number, slot);
		let _ = self.0.disputed_blocks.insert(block);
		block
	}

	pub fn ff(&mut self, branch_tag: u8, parent: Hash, block_number: BlockNumber, slot: impl Into<Slot>) -> Hash {
		let slot: Slot = slot.into();
		let hash = Hash::repeat_byte((block_number & 0xA0) as u8);
		let _ = self.add_block(hash, parent, slot, block_number);
		hash
	}

	pub fn set_heads(&mut self, heads: impl IntoIterator<Item=Hash>) {
		self.0.heads = heads.into_iter().collect();
	}

	pub fn init(mut self) -> ChainContent {
		self.0
	}

	fn make_header(parent_hash: Hash, slot: Slot, number: u32) -> Header {
		let digest = {
			let mut digest = Digest::default();
			let (vrf_output, vrf_proof) = garbage_vrf();
			digest.push(DigestItem::babe_pre_digest(PreDigest::SecondaryVRF(SecondaryVRFPreDigest {
				authority_index: 0,
				slot,
				vrf_output,
				vrf_proof,
			})));
			digest
		};

		Header { digest, extrinsics_root: Default::default(), number, state_root: Default::default(), parent_hash }
	}
}

/// Generalized sequence of the test, based on
/// the messages being sent out by the `fn finality_target`
/// Depends on a particular `target_hash`
/// that is passed to `finality_target` block number.
async fn test_skeleton(
	chain: &ChainContent,
	virtual_overseer: &mut VirtualOverseer,
	best_chain_containing_block: Option<Hash>,
	highest_approved_ancestor_block: Option<HighestApprovedAncestorBlock>,
	undisputed_chain: Option<Hash>,
) {
	let undisputed_chain = undisputed_chain.map(|x| (chain.number(x).unwrap().unwrap(), x));

	assert_matches!(
		overseer_recv(
			virtual_overseer
		).await,
		AllMessages::ChainSelection(ChainSelectionMessage::BestLeafContaining(target_hash, tx))
		=> {
			tx.send(best_chain_containing_block).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(
			virtual_overseer
		).await,
		AllMessages::ApprovalVoting(ApprovalVotingMessage::ApprovedAncestor(block_hash, block_number, tx))
		=> {
			tx.send(highest_approved_ancestor_block).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(
			virtual_overseer
		).await,
		AllMessages::DisputeCoordinator(
			DisputeCoordinatorMessage::DetermineUndisputedChain {
				base_number,
				block_descriptions,
				tx,
			}
		) => {
			tx.send(undisputed_chain).unwrap();
	});
}

/// Straight forward test case, where the test is not
/// for integrity, but for different block relation structures.
fn run_specialized_test_w_harness<F: FnOnce() -> CaseVars> (case_var_provider: F)
{
	test_harness(case_var_provider(), |test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
			finality_target_rx,
			case_vars:
				CaseVars {
					heads,
					chain,
					target_block,
					best_chain_containing_block,
					highest_approved_ancestor_block,
					undisputed_chain,
				},
			..
		} = test_harness;

		// XXX nice to have
		let highest_approved_ancestor_w_desc = chain.highest_approved_ancestors(target_block);

		// if let (Some(highest_approved_ancestor_w_desc), Some(highest_approved_ancestor_block)) =
		// 	(highest_approved_ancestor_w_desc, highest_approved_ancestor_block)
		// {
		// 	assert_eq!(
		// 		highest_approved_ancestor_block, highest_approved_ancestor_w_desc.hash,
		// 		"Derived block hash and provided do not match {:?} vs {:?}",
		// 		highest_approved_ancestor_block, highest_approved_ancestor_w_desc.hash,
		// 	);
		// }
		test_skeleton(
			&chain,
			&mut virtual_overseer,
			best_chain_containing_block,
			highest_approved_ancestor_w_desc,
			undisputed_chain
		).await;

		// TODO FIXME
		assert_matches!(finality_target_rx.await,
		 	Ok(
		 		hash_to_finalize,
			) => assert_eq!(undisputed_chain, hash_to_finalize));

		virtual_overseer
	});


}

/// All variables relevant for a test case.
#[derive(Clone, Debug)]
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

	let a1 = builder.ff_available(0xA0, head, 1, 1);
	let a2 = builder.ff_available(0xA0, a1, 2, 1);
	let a3 = builder.ff_available(0xA0, a2, 3, 1);
	let a4 = builder.ff(0xA0, a3, 4, 1);
	let a5 = builder.ff(0xA0, a4, 5, 1);
	let b1 = builder.ff_available(0xB0, a1, 2, 2);
	let b2 = builder.ff_available(0xB0, b1, 2, 2);
	let b3 = builder.ff_available(0xB0, b2, 2, 2);

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

	let a1 = builder.ff_available(0xA0, head, 1, 1);
	let a2 = builder.ff_disputed(0xA0, a1, 2, 1);
	let a3 = builder.ff_available(0xA0, a2, 3, 1);
	let b2 = builder.ff_available(0xB0, a1, 2, 2);
	let b3 = builder.ff_available(0xB0, b2, 3, 2);

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

	let a1 = builder.ff_available(0xA0, head, 1, 1);
	let a2 = builder.ff_disputed(0xA0, a1, 2, 1);
	let a3 = builder.ff_available(0xA0, a2, 3, 1);
	let b2 = builder.ff_available(0xB0, a1, 2, 2);
	let b3 = builder.ff_available(0xB0, b2, 3, 2);

	CaseVars {
		chain: builder.init(),
		heads: vec![a3, b3],
		target_block: A3,
		best_chain_containing_block: Some(A3),
		highest_approved_ancestor_block: Some(A3),
		undisputed_chain: None,
	}
}

/// ```raw
/// genesis -- 0xA1 --- 0xA2 --- 0xA3(disputed)
///               \
///                `- 0xB2 --- 0xB3
/// ```
fn chain_3() -> CaseVars {
	let mut head: Hash = ChainBuilder::GENESIS_HASH;
	let mut builder = ChainBuilder::new();

	let a1 = builder.ff_available(0xA0, head, 1, 1);
	let a2 = builder.ff_available(0xA0, a1, 2, 1);
	let a3 = builder.ff_disputed( 0xA0, a2, 3, 1);
	let b2 = builder.ff_available(0xB0, a1, 2, 2);
	let b3 = builder.ff_available(0xB0, b2, 3, 2);

	CaseVars {
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
