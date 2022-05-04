// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Everything required to run benchmarks of messages module, based on
//! `bridge_runtime_common::messages` implementation.

#![cfg(feature = "runtime-benchmarks")]

use crate::messages::{
	source::{FromBridgedChainMessagesDeliveryProof, FromThisChainMessagePayload},
	target::FromBridgedChainMessagesProof,
	AccountIdOf, BalanceOf, BridgedChain, CallOf, HashOf, MessageBridge, RawStorageProof,
	SignatureOf, SignerOf, ThisChain,
};

use bp_messages::{storage_keys, MessageData, MessageKey, MessagePayload};
use codec::Encode;
use frame_support::weights::{GetDispatchInfo, Weight};
use pallet_bridge_messages::benchmarking::{
	MessageDeliveryProofParams, MessageParams, MessageProofParams, ProofSize,
};
use sp_core::Hasher;
use sp_runtime::traits::{Header, IdentifyAccount, MaybeSerializeDeserialize, Zero};
use sp_std::{fmt::Debug, prelude::*};
use sp_trie::{record_all_keys, trie_types::TrieDBMutV1, LayoutV1, MemoryDB, Recorder, TrieMut};

/// Prepare outbound message for the `send_message` call.
pub fn prepare_outbound_message<B>(
	params: MessageParams<AccountIdOf<ThisChain<B>>>,
) -> FromThisChainMessagePayload
where
	B: MessageBridge,
	BalanceOf<ThisChain<B>>: From<u64>,
{
	vec![0; params.size as usize]
}

/// Prepare proof of messages for the `receive_messages_proof` call.
///
/// In addition to returning valid messages proof, environment is prepared to verify this message
/// proof.
pub fn prepare_message_proof<R, BI, FI, B, BH, BHH>(
	params: MessageProofParams,
) -> (FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>, Weight)
where
	R: frame_system::Config<AccountId = AccountIdOf<ThisChain<B>>>
		+ pallet_balances::Config<BI, Balance = BalanceOf<ThisChain<B>>>
		+ pallet_bridge_grandpa::Config<FI>,
	R::BridgedChain: bp_runtime::Chain<Header = BH>,
	B: MessageBridge,
	BI: 'static,
	FI: 'static,
	BH: Header<Hash = HashOf<BridgedChain<B>>>,
	BHH: Hasher<Out = HashOf<BridgedChain<B>>>,
	AccountIdOf<ThisChain<B>>: PartialEq + sp_std::fmt::Debug,
	AccountIdOf<BridgedChain<B>>: From<[u8; 32]>,
	BalanceOf<ThisChain<B>>: Debug + MaybeSerializeDeserialize,
	CallOf<ThisChain<B>>: From<frame_system::Call<R>> + GetDispatchInfo,
	HashOf<BridgedChain<B>>: Copy + Default,
	SignatureOf<ThisChain<B>>: From<sp_core::ed25519::Signature>,
	SignerOf<ThisChain<B>>: Clone
		+ From<sp_core::ed25519::Public>
		+ IdentifyAccount<AccountId = AccountIdOf<ThisChain<B>>>,
{
	let message_payload = match params.size {
		ProofSize::Minimal(ref size) => vec![0u8; *size as _],
		_ => vec![],
	};

	// finally - prepare storage proof and update environment
	let (state_root, storage_proof) =
		prepare_messages_storage_proof::<B, BHH>(&params, message_payload);
	let bridged_header_hash = insert_bridged_chain_header::<R, FI, B, BH>(state_root);

	(
		FromBridgedChainMessagesProof {
			bridged_header_hash,
			storage_proof,
			lane: params.lane,
			nonces_start: *params.message_nonces.start(),
			nonces_end: *params.message_nonces.end(),
		},
		0,
	)
}

/// Prepare proof of messages delivery for the `receive_messages_delivery_proof` call.
pub fn prepare_message_delivery_proof<R, FI, B, BH, BHH>(
	params: MessageDeliveryProofParams<AccountIdOf<ThisChain<B>>>,
) -> FromBridgedChainMessagesDeliveryProof<HashOf<BridgedChain<B>>>
where
	R: pallet_bridge_grandpa::Config<FI>,
	R::BridgedChain: bp_runtime::Chain<Header = BH>,
	FI: 'static,
	B: MessageBridge,
	BH: Header<Hash = HashOf<BridgedChain<B>>>,
	BHH: Hasher<Out = HashOf<BridgedChain<B>>>,
	HashOf<BridgedChain<B>>: Copy + Default,
{
	// prepare Bridged chain storage with inbound lane state
	let storage_key =
		storage_keys::inbound_lane_data_key(B::BRIDGED_MESSAGES_PALLET_NAME, &params.lane).0;
	let mut root = Default::default();
	let mut mdb = MemoryDB::default();
	{
		let mut trie = TrieDBMutV1::<BHH>::new(&mut mdb, &mut root);
		trie.insert(&storage_key, &params.inbound_lane_data.encode())
			.map_err(|_| "TrieMut::insert has failed")
			.expect("TrieMut::insert should not fail in benchmarks");
	}
	root = grow_trie(root, &mut mdb, params.size);

	// generate storage proof to be delivered to This chain
	let mut proof_recorder = Recorder::<BHH::Out>::new();
	record_all_keys::<LayoutV1<BHH>, _>(&mdb, &root, &mut proof_recorder)
		.map_err(|_| "record_all_keys has failed")
		.expect("record_all_keys should not fail in benchmarks");
	let storage_proof = proof_recorder.drain().into_iter().map(|n| n.data.to_vec()).collect();

	// finally insert header with given state root to our storage
	let bridged_header_hash = insert_bridged_chain_header::<R, FI, B, BH>(root);

	FromBridgedChainMessagesDeliveryProof {
		bridged_header_hash: bridged_header_hash.into(),
		storage_proof,
		lane: params.lane,
	}
}

/// Prepare storage proof of given messages.
///
/// Returns state trie root and nodes with prepared messages.
fn prepare_messages_storage_proof<B, BHH>(
	params: &MessageProofParams,
	message_payload: MessagePayload,
) -> (HashOf<BridgedChain<B>>, RawStorageProof)
where
	B: MessageBridge,
	BHH: Hasher<Out = HashOf<BridgedChain<B>>>,
	HashOf<BridgedChain<B>>: Copy + Default,
{
	// prepare Bridged chain storage with messages and (optionally) outbound lane state
	let message_count =
		params.message_nonces.end().saturating_sub(*params.message_nonces.start()) + 1;
	let mut storage_keys = Vec::with_capacity(message_count as usize + 1);
	let mut root = Default::default();
	let mut mdb = MemoryDB::default();
	{
		let mut trie = TrieDBMutV1::<BHH>::new(&mut mdb, &mut root);

		// insert messages
		for nonce in params.message_nonces.clone() {
			let message_key = MessageKey { lane_id: params.lane, nonce };
			let message_data = MessageData {
				fee: BalanceOf::<BridgedChain<B>>::from(0),
				payload: message_payload.clone(),
			};
			let storage_key = storage_keys::message_key(
				B::BRIDGED_MESSAGES_PALLET_NAME,
				&message_key.lane_id,
				message_key.nonce,
			)
			.0;
			trie.insert(&storage_key, &message_data.encode())
				.map_err(|_| "TrieMut::insert has failed")
				.expect("TrieMut::insert should not fail in benchmarks");
			storage_keys.push(storage_key);
		}

		// insert outbound lane state
		if let Some(ref outbound_lane_data) = params.outbound_lane_data {
			let storage_key =
				storage_keys::outbound_lane_data_key(B::BRIDGED_MESSAGES_PALLET_NAME, &params.lane)
					.0;
			trie.insert(&storage_key, &outbound_lane_data.encode())
				.map_err(|_| "TrieMut::insert has failed")
				.expect("TrieMut::insert should not fail in benchmarks");
			storage_keys.push(storage_key);
		}
	}
	root = grow_trie(root, &mut mdb, params.size);

	// generate storage proof to be delivered to This chain
	let mut proof_recorder = Recorder::<BHH::Out>::new();
	record_all_keys::<LayoutV1<BHH>, _>(&mdb, &root, &mut proof_recorder)
		.map_err(|_| "record_all_keys has failed")
		.expect("record_all_keys should not fail in benchmarks");
	let storage_proof = proof_recorder.drain().into_iter().map(|n| n.data.to_vec()).collect();

	(root, storage_proof)
}

/// Insert Bridged chain header with given state root into storage of GRANDPA pallet at This chain.
fn insert_bridged_chain_header<R, FI, B, BH>(
	state_root: HashOf<BridgedChain<B>>,
) -> HashOf<BridgedChain<B>>
where
	R: pallet_bridge_grandpa::Config<FI>,
	R::BridgedChain: bp_runtime::Chain<Header = BH>,
	FI: 'static,
	B: MessageBridge,
	BH: Header<Hash = HashOf<BridgedChain<B>>>,
	HashOf<BridgedChain<B>>: Default,
{
	let bridged_header = BH::new(
		Zero::zero(),
		Default::default(),
		state_root,
		Default::default(),
		Default::default(),
	);
	let bridged_header_hash = bridged_header.hash();
	pallet_bridge_grandpa::initialize_for_benchmarks::<R, FI>(bridged_header);
	bridged_header_hash
}

/// Populate trie with dummy keys+values until trie has at least given size.
fn grow_trie<H: Hasher>(mut root: H::Out, mdb: &mut MemoryDB<H>, trie_size: ProofSize) -> H::Out {
	let (iterations, leaf_size, minimal_trie_size) = match trie_size {
		ProofSize::Minimal(_) => return root,
		ProofSize::HasLargeLeaf(size) => (1, size, size),
		ProofSize::HasExtraNodes(size) => (8, 1, size),
	};

	let mut key_index = 0;
	loop {
		// generate storage proof to be delivered to This chain
		let mut proof_recorder = Recorder::<H::Out>::new();
		record_all_keys::<LayoutV1<H>, _>(mdb, &root, &mut proof_recorder)
			.map_err(|_| "record_all_keys has failed")
			.expect("record_all_keys should not fail in benchmarks");
		let size: usize = proof_recorder.drain().into_iter().map(|n| n.data.len()).sum();
		if size > minimal_trie_size as _ {
			return root
		}

		let mut trie = TrieDBMutV1::<H>::from_existing(mdb, &mut root)
			.map_err(|_| "TrieDBMutV1::from_existing has failed")
			.expect("TrieDBMutV1::from_existing should not fail in benchmarks");
		for _ in 0..iterations {
			trie.insert(&key_index.encode(), &vec![42u8; leaf_size as _])
				.map_err(|_| "TrieMut::insert has failed")
				.expect("TrieMut::insert should not fail in benchmarks");
			key_index += 1;
		}
		trie.commit();
	}
}
