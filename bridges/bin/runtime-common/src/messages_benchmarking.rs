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
use bp_runtime::{messages::DispatchFeePayment, ChainId};
use codec::Encode;
use ed25519_dalek::{PublicKey, SecretKey, Signer, KEYPAIR_LENGTH, SECRET_KEY_LENGTH};
use frame_support::{
	traits::Currency,
	weights::{GetDispatchInfo, Weight},
};
use pallet_bridge_messages::benchmarking::{
	MessageDeliveryProofParams, MessageParams, MessageProofParams, ProofSize,
};
use sp_core::Hasher;
use sp_runtime::traits::{Header, IdentifyAccount, MaybeSerializeDeserialize, Zero};
use sp_std::{fmt::Debug, prelude::*};
use sp_trie::{record_all_keys, trie_types::TrieDBMutV1, LayoutV1, MemoryDB, Recorder, TrieMut};
use sp_version::RuntimeVersion;

/// Return this chain account, used to dispatch message.
pub fn dispatch_account<B>() -> AccountIdOf<ThisChain<B>>
where
	B: MessageBridge,
	SignerOf<ThisChain<B>>:
		From<sp_core::ed25519::Public> + IdentifyAccount<AccountId = AccountIdOf<ThisChain<B>>>,
{
	let this_raw_public = PublicKey::from(&dispatch_account_secret());
	let this_public: SignerOf<ThisChain<B>> =
		sp_core::ed25519::Public::from_raw(this_raw_public.to_bytes()).into();
	this_public.into_account()
}

/// Return public key of this chain account, used to dispatch message.
pub fn dispatch_account_secret() -> SecretKey {
	// key from the repo example (https://docs.rs/ed25519-dalek/1.0.1/ed25519_dalek/struct.SecretKey.html)
	SecretKey::from_bytes(&[
		157, 097, 177, 157, 239, 253, 090, 096, 186, 132, 074, 244, 146, 236, 044, 196, 068, 073,
		197, 105, 123, 050, 105, 025, 112, 059, 172, 003, 028, 174, 127, 096,
	])
	.expect("harcoded key is valid")
}

/// Prepare outbound message for the `send_message` call.
pub fn prepare_outbound_message<B>(
	params: MessageParams<AccountIdOf<ThisChain<B>>>,
) -> FromThisChainMessagePayload<B>
where
	B: MessageBridge,
	BalanceOf<ThisChain<B>>: From<u64>,
{
	let message_payload = vec![0; params.size as usize];
	let dispatch_origin = bp_message_dispatch::CallOrigin::SourceAccount(params.sender_account);

	FromThisChainMessagePayload::<B> {
		spec_version: 0,
		weight: params.size as _,
		origin: dispatch_origin,
		call: message_payload,
		dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
	}
}

/// Prepare proof of messages for the `receive_messages_proof` call.
///
/// In addition to returning valid messages proof, environment is prepared to verify this message
/// proof.
pub fn prepare_message_proof<R, BI, FI, B, BH, BHH>(
	params: MessageProofParams,
	version: &RuntimeVersion,
	endow_amount: BalanceOf<ThisChain<B>>,
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
	// we'll be dispatching the same call at This chain
	let remark = match params.size {
		ProofSize::Minimal(ref size) => vec![0u8; *size as _],
		_ => vec![],
	};
	let call: CallOf<ThisChain<B>> = frame_system::Call::remark { remark }.into();
	let call_weight = call.get_dispatch_info().weight;

	// message payload needs to be signed, because we use `TargetAccount` call origin
	// (which is 'heaviest' to verify)
	let bridged_account_id: AccountIdOf<BridgedChain<B>> = [0u8; 32].into();
	let (this_raw_public, this_raw_signature) = ed25519_sign(
		&call,
		&bridged_account_id,
		version.spec_version,
		B::BRIDGED_CHAIN_ID,
		B::THIS_CHAIN_ID,
	);
	let this_public: SignerOf<ThisChain<B>> =
		sp_core::ed25519::Public::from_raw(this_raw_public).into();
	let this_signature: SignatureOf<ThisChain<B>> =
		sp_core::ed25519::Signature::from_raw(this_raw_signature).into();

	// if dispatch fee is paid at this chain, endow relayer account
	if params.dispatch_fee_payment == DispatchFeePayment::AtTargetChain {
		assert_eq!(this_public.clone().into_account(), dispatch_account::<B>());
		pallet_balances::Pallet::<R, BI>::make_free_balance_be(
			&this_public.clone().into_account(),
			endow_amount,
		);
	}

	// prepare message payload that is stored in the Bridged chain storage
	let message_payload = bp_message_dispatch::MessagePayload {
		spec_version: version.spec_version,
		weight: call_weight,
		origin: bp_message_dispatch::CallOrigin::<
			AccountIdOf<BridgedChain<B>>,
			SignerOf<ThisChain<B>>,
			SignatureOf<ThisChain<B>>,
		>::TargetAccount(bridged_account_id, this_public, this_signature),
		dispatch_fee_payment: params.dispatch_fee_payment.clone(),
		call: call.encode(),
	}
	.encode();

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
		call_weight
			.checked_mul(
				params.message_nonces.end().saturating_sub(*params.message_nonces.start()) + 1,
			)
			.expect("too many messages requested by benchmark"),
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

/// Generate ed25519 signature to be used in
/// `pallet_brdige_call_dispatch::CallOrigin::TargetAccount`.
///
/// Returns public key of the signer and the signature itself.
fn ed25519_sign(
	target_call: &impl Encode,
	source_account_id: &impl Encode,
	target_spec_version: u32,
	source_chain_id: ChainId,
	target_chain_id: ChainId,
) -> ([u8; 32], [u8; 64]) {
	let target_secret = dispatch_account_secret();
	let target_public: PublicKey = (&target_secret).into();

	let mut target_pair_bytes = [0u8; KEYPAIR_LENGTH];
	target_pair_bytes[..SECRET_KEY_LENGTH].copy_from_slice(&target_secret.to_bytes());
	target_pair_bytes[SECRET_KEY_LENGTH..].copy_from_slice(&target_public.to_bytes());
	let target_pair =
		ed25519_dalek::Keypair::from_bytes(&target_pair_bytes).expect("hardcoded pair is valid");

	let signature_message = pallet_bridge_dispatch::account_ownership_digest(
		target_call,
		source_account_id,
		target_spec_version,
		source_chain_id,
		target_chain_id,
	);
	let target_origin_signature = target_pair
		.try_sign(&signature_message)
		.expect("Ed25519 try_sign should not fail in benchmarks");

	(target_public.to_bytes(), target_origin_signature.to_bytes())
}

/// Populate trie with dummy keys+values until trie has at least given size.
fn grow_trie<H: Hasher>(mut root: H::Out, mdb: &mut MemoryDB<H>, trie_size: ProofSize) -> H::Out {
	let (iterations, leaf_size, minimal_trie_size) = match trie_size {
		ProofSize::Minimal(_) => return root,
		ProofSize::HasLargeLeaf(size) => (1, size, size),
		ProofSize::HasExtraNodes(size) => (8, 1, size),
	};

	let mut key_index = 0u32;
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
