// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Everything required to run benchmarks of message-lanes, based on
//! `bridge_runtime_common::messages` implementation.

#![cfg(feature = "runtime-benchmarks")]

use crate::messages::{
	source::FromBridgedChainMessagesDeliveryProof, target::FromBridgedChainMessagesProof, AccountIdOf, BalanceOf,
	BridgedChain, HashOf, MessageBridge, ThisChain,
};

use bp_message_lane::{LaneId, MessageData, MessageKey, MessagePayload};
use codec::Encode;
use ed25519_dalek::{PublicKey, SecretKey, Signer, KEYPAIR_LENGTH, SECRET_KEY_LENGTH};
use frame_support::weights::Weight;
use pallet_message_lane::benchmarking::{MessageDeliveryProofParams, MessageProofParams, ProofSize};
use sp_core::Hasher;
use sp_runtime::traits::Header;
use sp_std::prelude::*;
use sp_trie::{record_all_keys, trie_types::TrieDBMut, Layout, MemoryDB, Recorder, TrieMut};

/// Generate ed25519 signature to be used in `pallet_brdige_call_dispatch::CallOrigin::TargetAccount`.
///
/// Returns public key of the signer and the signature itself.
pub fn ed25519_sign(target_call: &impl Encode, source_account_id: &impl Encode) -> ([u8; 32], [u8; 64]) {
	// key from the repo example (https://docs.rs/ed25519-dalek/1.0.1/ed25519_dalek/struct.SecretKey.html)
	let target_secret = SecretKey::from_bytes(&[
		157, 097, 177, 157, 239, 253, 090, 096, 186, 132, 074, 244, 146, 236, 044, 196, 068, 073, 197, 105, 123, 050,
		105, 025, 112, 059, 172, 003, 028, 174, 127, 096,
	])
	.expect("harcoded key is valid");
	let target_public: PublicKey = (&target_secret).into();

	let mut target_pair_bytes = [0u8; KEYPAIR_LENGTH];
	target_pair_bytes[..SECRET_KEY_LENGTH].copy_from_slice(&target_secret.to_bytes());
	target_pair_bytes[SECRET_KEY_LENGTH..].copy_from_slice(&target_public.to_bytes());
	let target_pair = ed25519_dalek::Keypair::from_bytes(&target_pair_bytes).expect("hardcoded pair is valid");

	let mut signature_message = Vec::new();
	target_call.encode_to(&mut signature_message);
	source_account_id.encode_to(&mut signature_message);
	let target_origin_signature = target_pair
		.try_sign(&signature_message)
		.expect("Ed25519 try_sign should not fail in benchmarks");

	(target_public.to_bytes(), target_origin_signature.to_bytes())
}

/// Prepare proof of messages for the `receive_messages_proof` call.
pub fn prepare_message_proof<B, H, R, MM, ML, MH>(
	params: MessageProofParams,
	make_bridged_message_storage_key: MM,
	make_bridged_outbound_lane_data_key: ML,
	make_bridged_header: MH,
	message_dispatch_weight: Weight,
	message_payload: MessagePayload,
) -> (FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>, Weight)
where
	B: MessageBridge,
	H: Hasher,
	R: pallet_substrate_bridge::Config,
	<R::BridgedChain as bp_runtime::Chain>::Hash: Into<HashOf<BridgedChain<B>>>,
	MM: Fn(MessageKey) -> Vec<u8>,
	ML: Fn(LaneId) -> Vec<u8>,
	MH: Fn(H::Out) -> <R::BridgedChain as bp_runtime::Chain>::Header,
{
	// prepare Bridged chain storage with messages and (optionally) outbound lane state
	let message_count = params
		.message_nonces
		.end()
		.saturating_sub(*params.message_nonces.start())
		+ 1;
	let mut storage_keys = Vec::with_capacity(message_count as usize + 1);
	let mut root = Default::default();
	let mut mdb = MemoryDB::default();
	{
		let mut trie = TrieDBMut::<H>::new(&mut mdb, &mut root);

		// insert messages
		for nonce in params.message_nonces.clone() {
			let message_key = MessageKey {
				lane_id: params.lane,
				nonce,
			};
			let message_data = MessageData {
				fee: BalanceOf::<BridgedChain<B>>::from(0),
				payload: message_payload.clone(),
			};
			let storage_key = make_bridged_message_storage_key(message_key);
			trie.insert(&storage_key, &message_data.encode())
				.map_err(|_| "TrieMut::insert has failed")
				.expect("TrieMut::insert should not fail in benchmarks");
			storage_keys.push(storage_key);
		}

		// insert outbound lane state
		if let Some(outbound_lane_data) = params.outbound_lane_data {
			let storage_key = make_bridged_outbound_lane_data_key(params.lane);
			trie.insert(&storage_key, &outbound_lane_data.encode())
				.map_err(|_| "TrieMut::insert has failed")
				.expect("TrieMut::insert should not fail in benchmarks");
			storage_keys.push(storage_key);
		}
	}
	root = grow_trie(root, &mut mdb, params.size);

	// generate storage proof to be delivered to This chain
	let mut proof_recorder = Recorder::<H::Out>::new();
	record_all_keys::<Layout<H>, _>(&mdb, &root, &mut proof_recorder)
		.map_err(|_| "record_all_keys has failed")
		.expect("record_all_keys should not fail in benchmarks");
	let storage_proof = proof_recorder.drain().into_iter().map(|n| n.data.to_vec()).collect();

	// prepare Bridged chain header and insert it into the Substrate pallet
	let bridged_header = make_bridged_header(root);
	let bridged_header_hash = bridged_header.hash();
	pallet_substrate_bridge::initialize_for_benchmarks::<R>(bridged_header);

	(
		FromBridgedChainMessagesProof {
			bridged_header_hash: bridged_header_hash.into(),
			storage_proof,
			lane: params.lane,
			nonces_start: *params.message_nonces.start(),
			nonces_end: *params.message_nonces.end(),
		},
		message_dispatch_weight
			.checked_mul(message_count)
			.expect("too many messages requested by benchmark"),
	)
}

/// Prepare proof of messages delivery for the `receive_messages_delivery_proof` call.
pub fn prepare_message_delivery_proof<B, H, R, ML, MH>(
	params: MessageDeliveryProofParams<AccountIdOf<ThisChain<B>>>,
	make_bridged_inbound_lane_data_key: ML,
	make_bridged_header: MH,
) -> FromBridgedChainMessagesDeliveryProof<HashOf<BridgedChain<B>>>
where
	B: MessageBridge,
	H: Hasher,
	R: pallet_substrate_bridge::Config,
	<R::BridgedChain as bp_runtime::Chain>::Hash: Into<HashOf<BridgedChain<B>>>,
	ML: Fn(LaneId) -> Vec<u8>,
	MH: Fn(H::Out) -> <R::BridgedChain as bp_runtime::Chain>::Header,
{
	// prepare Bridged chain storage with inbound lane state
	let storage_key = make_bridged_inbound_lane_data_key(params.lane);
	let mut root = Default::default();
	let mut mdb = MemoryDB::default();
	{
		let mut trie = TrieDBMut::<H>::new(&mut mdb, &mut root);
		trie.insert(&storage_key, &params.inbound_lane_data.encode())
			.map_err(|_| "TrieMut::insert has failed")
			.expect("TrieMut::insert should not fail in benchmarks");
	}
	root = grow_trie(root, &mut mdb, params.size);

	// generate storage proof to be delivered to This chain
	let mut proof_recorder = Recorder::<H::Out>::new();
	record_all_keys::<Layout<H>, _>(&mdb, &root, &mut proof_recorder)
		.map_err(|_| "record_all_keys has failed")
		.expect("record_all_keys should not fail in benchmarks");
	let storage_proof = proof_recorder.drain().into_iter().map(|n| n.data.to_vec()).collect();

	// prepare Bridged chain header and insert it into the Substrate pallet
	let bridged_header = make_bridged_header(root);
	let bridged_header_hash = bridged_header.hash();
	pallet_substrate_bridge::initialize_for_benchmarks::<R>(bridged_header);

	FromBridgedChainMessagesDeliveryProof {
		bridged_header_hash: bridged_header_hash.into(),
		storage_proof,
		lane: params.lane,
	}
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
		record_all_keys::<Layout<H>, _>(mdb, &root, &mut proof_recorder)
			.map_err(|_| "record_all_keys has failed")
			.expect("record_all_keys should not fail in benchmarks");
		let size: usize = proof_recorder.drain().into_iter().map(|n| n.data.len()).sum();
		if size > minimal_trie_size as _ {
			return root;
		}

		let mut trie = TrieDBMut::<H>::from_existing(mdb, &mut root)
			.map_err(|_| "TrieDBMut::from_existing has failed")
			.expect("TrieDBMut::from_existing should not fail in benchmarks");
		for _ in 0..iterations {
			trie.insert(&key_index.encode(), &vec![42u8; leaf_size as _])
				.map_err(|_| "TrieMut::insert has failed")
				.expect("TrieMut::insert should not fail in benchmarks");
			key_index += 1;
		}
		trie.commit();
	}
}
