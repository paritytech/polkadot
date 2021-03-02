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

//! Message lane pallet benchmarking.

use crate::weights_ext::EXPECTED_DEFAULT_MESSAGE_LENGTH;
use crate::{inbound_lane::InboundLaneStorage, inbound_lane_storage, outbound_lane, Call, Instance};

use bp_message_lane::{
	source_chain::TargetHeaderChain, target_chain::SourceHeaderChain, InboundLaneData, LaneId, MessageData,
	MessageNonce, OutboundLaneData, UnrewardedRelayersState,
};
use frame_benchmarking::{account, benchmarks_instance};
use frame_support::{traits::Get, weights::Weight};
use frame_system::RawOrigin;
use sp_std::{collections::btree_map::BTreeMap, convert::TryInto, ops::RangeInclusive, prelude::*};

/// Fee paid by submitter for single message delivery.
pub const MESSAGE_FEE: u64 = 10_000_000_000;

const SEED: u32 = 0;

/// Module we're benchmarking here.
pub struct Module<T: Config<I>, I: crate::Instance>(crate::Module<T, I>);

/// Proof size requirements.
pub enum ProofSize {
	/// The proof is expected to be minimal. If value size may be changed, then it is expected to
	/// have given size.
	Minimal(u32),
	/// The proof is expected to have at least given size and grow by increasing number of trie nodes
	/// included in the proof.
	HasExtraNodes(u32),
	/// The proof is expected to have at least given size and grow by increasing value that is stored
	/// in the trie.
	HasLargeLeaf(u32),
}

/// Benchmark-specific message parameters.
pub struct MessageParams<ThisAccountId> {
	/// Size of the message payload.
	pub size: u32,
	/// Message sender account.
	pub sender_account: ThisAccountId,
}

/// Benchmark-specific message proof parameters.
pub struct MessageProofParams {
	/// Id of the lane.
	pub lane: LaneId,
	/// Range of messages to include in the proof.
	pub message_nonces: RangeInclusive<MessageNonce>,
	/// If `Some`, the proof needs to include this outbound lane data.
	pub outbound_lane_data: Option<OutboundLaneData>,
	/// Proof size requirements.
	pub size: ProofSize,
}

/// Benchmark-specific message delivery proof parameters.
pub struct MessageDeliveryProofParams<ThisChainAccountId> {
	/// Id of the lane.
	pub lane: LaneId,
	/// The proof needs to include this inbound lane data.
	pub inbound_lane_data: InboundLaneData<ThisChainAccountId>,
	/// Proof size requirements.
	pub size: ProofSize,
}

/// Trait that must be implemented by runtime.
pub trait Config<I: Instance>: crate::Config<I> {
	/// Lane id to use in benchmarks.
	fn bench_lane_id() -> LaneId {
		Default::default()
	}
	/// Get maximal size of the message payload.
	fn maximal_message_size() -> u32;
	/// Return id of relayer account at the bridged chain.
	fn bridged_relayer_id() -> Self::InboundRelayer;
	/// Return balance of given account.
	fn account_balance(account: &Self::AccountId) -> Self::OutboundMessageFee;
	/// Create given account and give it enough balance for test purposes.
	fn endow_account(account: &Self::AccountId);
	/// Prepare message to send over lane.
	fn prepare_outbound_message(
		params: MessageParams<Self::AccountId>,
	) -> (Self::OutboundPayload, Self::OutboundMessageFee);
	/// Prepare messages proof to receive by the module.
	fn prepare_message_proof(
		params: MessageProofParams,
	) -> (
		<Self::SourceHeaderChain as SourceHeaderChain<Self::InboundMessageFee>>::MessagesProof,
		Weight,
	);
	/// Prepare messages delivery proof to receive by the module.
	fn prepare_message_delivery_proof(
		params: MessageDeliveryProofParams<Self::AccountId>,
	) -> <Self::TargetHeaderChain as TargetHeaderChain<Self::OutboundPayload, Self::AccountId>>::MessagesDeliveryProof;
}

benchmarks_instance! {
	//
	// Benchmarks that are used directly by the runtime.
	//

	// Benchmark `send_message` extrinsic with the worst possible conditions:
	// * outbound lane already has state, so it needs to be read and decoded;
	// * relayers fund account does not exists (in practice it needs to exist in production environment);
	// * maximal number of messages is being pruned during the call;
	// * message size is minimal for the target chain.
	//
	// Result of this benchmark is used as a base weight for `send_message` call. Then the 'message weight'
	// (estimated using `send_half_maximal_message_worst_case` and `send_maximal_message_worst_case`) is
	// added.
	send_minimal_message_worst_case {
		let lane_id = T::bench_lane_id();
		let sender = account("sender", 0, SEED);
		T::endow_account(&sender);

		// 'send' messages that are to be pruned when our message is sent
		for _nonce in 1..=T::MaxMessagesToPruneAtOnce::get() {
			send_regular_message::<T, I>();
		}
		confirm_message_delivery::<T, I>(T::MaxMessagesToPruneAtOnce::get());

		let (payload, fee) = T::prepare_outbound_message(MessageParams {
			size: 0,
			sender_account: sender.clone(),
		});
	}: send_message(RawOrigin::Signed(sender), lane_id, payload, fee)
	verify {
		assert_eq!(
			crate::Module::<T, I>::outbound_latest_generated_nonce(T::bench_lane_id()),
			T::MaxMessagesToPruneAtOnce::get() + 1,
		);
	}

	// Benchmark `send_message` extrinsic with the worst possible conditions:
	// * outbound lane already has state, so it needs to be read and decoded;
	// * relayers fund account does not exists (in practice it needs to exist in production environment);
	// * maximal number of messages is being pruned during the call;
	// * message size is 1KB.
	//
	// With single KB of message size, the weight of the call is increased (roughly) by
	// `(send_16_kb_message_worst_case - send_1_kb_message_worst_case) / 15`.
	send_1_kb_message_worst_case {
		let lane_id = T::bench_lane_id();
		let sender = account("sender", 0, SEED);
		T::endow_account(&sender);

		// 'send' messages that are to be pruned when our message is sent
		for _nonce in 1..=T::MaxMessagesToPruneAtOnce::get() {
			send_regular_message::<T, I>();
		}
		confirm_message_delivery::<T, I>(T::MaxMessagesToPruneAtOnce::get());

		let size = 1024;
		assert!(
			T::maximal_message_size() > size,
			"This benchmark can only be used with runtime that accepts 1KB messages",
		);

		let (payload, fee) = T::prepare_outbound_message(MessageParams {
			size,
			sender_account: sender.clone(),
		});
	}: send_message(RawOrigin::Signed(sender), lane_id, payload, fee)
	verify {
		assert_eq!(
			crate::Module::<T, I>::outbound_latest_generated_nonce(T::bench_lane_id()),
			T::MaxMessagesToPruneAtOnce::get() + 1,
		);
	}

	// Benchmark `send_message` extrinsic with the worst possible conditions:
	// * outbound lane already has state, so it needs to be read and decoded;
	// * relayers fund account does not exists (in practice it needs to exist in production environment);
	// * maximal number of messages is being pruned during the call;
	// * message size is 16KB.
	//
	// With single KB of message size, the weight of the call is increased (roughly) by
	// `(send_16_kb_message_worst_case - send_1_kb_message_worst_case) / 15`.
	send_16_kb_message_worst_case {
		let lane_id = T::bench_lane_id();
		let sender = account("sender", 0, SEED);
		T::endow_account(&sender);

		// 'send' messages that are to be pruned when our message is sent
		for _nonce in 1..=T::MaxMessagesToPruneAtOnce::get() {
			send_regular_message::<T, I>();
		}
		confirm_message_delivery::<T, I>(T::MaxMessagesToPruneAtOnce::get());

		let size = 16 * 1024;
		assert!(
			T::maximal_message_size() > size,
			"This benchmark can only be used with runtime that accepts 16KB messages",
		);

		let (payload, fee) = T::prepare_outbound_message(MessageParams {
			size,
			sender_account: sender.clone(),
		});
	}: send_message(RawOrigin::Signed(sender), lane_id, payload, fee)
	verify {
		assert_eq!(
			crate::Module::<T, I>::outbound_latest_generated_nonce(T::bench_lane_id()),
			T::MaxMessagesToPruneAtOnce::get() + 1,
		);
	}

	// Benchmark `increase_message_fee` with following conditions:
	// * message has maximal message;
	// * submitter account is killed because its balance is less than ED after payment.
	increase_message_fee {
		let sender = account("sender", 42, SEED);
		T::endow_account(&sender);

		let additional_fee = T::account_balance(&sender);
		let lane_id = T::bench_lane_id();
		let nonce = 1;

		send_regular_message_with_payload::<T, I>(vec![42u8; T::maximal_message_size() as _]);
	}: increase_message_fee(RawOrigin::Signed(sender.clone()), lane_id, nonce, additional_fee)
	verify {
		assert_eq!(T::account_balance(&sender), 0.into());
	}

	// Benchmark `receive_messages_proof` extrinsic with single minimal-weight message and following conditions:
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// This is base benchmark for all other message delivery benchmarks.
	receive_single_message_proof {
		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=21,
			outbound_lane_data: None,
			size: ProofSize::Minimal(EXPECTED_DEFAULT_MESSAGE_LENGTH),
		});
	}: receive_messages_proof(RawOrigin::Signed(relayer_id_on_target), relayer_id_on_source, proof, 1, dispatch_weight)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			21,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with two minimal-weight messages and following conditions:
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// The weight of single message delivery could be approximated as
	// `weight(receive_two_messages_proof) - weight(receive_single_message_proof)`.
	// This won't be super-accurate if message has non-zero dispatch weight, but estimation should
	// be close enough to real weight.
	receive_two_messages_proof {
		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=22,
			outbound_lane_data: None,
			size: ProofSize::Minimal(EXPECTED_DEFAULT_MESSAGE_LENGTH),
		});
	}: receive_messages_proof(RawOrigin::Signed(relayer_id_on_target), relayer_id_on_source, proof, 2, dispatch_weight)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			22,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with single minimal-weight message and following conditions:
	// * proof includes outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// The weight of outbound lane state delivery would be
	// `weight(receive_single_message_proof_with_outbound_lane_state) - weight(receive_single_message_proof)`.
	// This won't be super-accurate if message has non-zero dispatch weight, but estimation should
	// be close enough to real weight.
	receive_single_message_proof_with_outbound_lane_state {
		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=21,
			outbound_lane_data: Some(OutboundLaneData {
				oldest_unpruned_nonce: 21,
				latest_received_nonce: 20,
				latest_generated_nonce: 21,
			}),
			size: ProofSize::Minimal(EXPECTED_DEFAULT_MESSAGE_LENGTH),
		});
	}: receive_messages_proof(RawOrigin::Signed(relayer_id_on_target), relayer_id_on_source, proof, 1, dispatch_weight)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			21,
		);
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_confirmed_nonce(T::bench_lane_id()),
			20,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with single minimal-weight message and following conditions:
	// * the proof has many redundand trie nodes with total size of approximately 1KB;
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// With single KB of messages proof, the weight of the call is increased (roughly) by
	// `(receive_single_message_proof_16KB - receive_single_message_proof_1_kb) / 15`.
	receive_single_message_proof_1_kb {
		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=21,
			outbound_lane_data: None,
			size: ProofSize::HasExtraNodes(1024),
		});
	}: receive_messages_proof(RawOrigin::Signed(relayer_id_on_target), relayer_id_on_source, proof, 1, dispatch_weight)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			21,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with single minimal-weight message and following conditions:
	// * the proof has many redundand trie nodes with total size of approximately 16KB;
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// Size of proof grows because it contains extra trie nodes in it.
	//
	// With single KB of messages proof, the weight of the call is increased (roughly) by
	// `(receive_single_message_proof_16KB - receive_single_message_proof) / 15`.
	receive_single_message_proof_16_kb {
		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=21,
			outbound_lane_data: None,
			size: ProofSize::HasExtraNodes(16 * 1024),
		});
	}: receive_messages_proof(RawOrigin::Signed(relayer_id_on_target), relayer_id_on_source, proof, 1, dispatch_weight)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			21,
		);
	}

	// Benchmark `receive_messages_delivery_proof` extrinsic with following conditions:
	// * single relayer is rewarded for relaying single message;
	// * relayer account does not exist (in practice it needs to exist in production environment).
	//
	// This is base benchmark for all other confirmations delivery benchmarks.
	receive_delivery_proof_for_single_message {
		let relayers_fund_id = crate::Module::<T, I>::relayer_fund_account_id();
		let relayer_id: T::AccountId = account("relayer", 0, SEED);
		let relayer_balance = T::account_balance(&relayer_id);
		T::endow_account(&relayers_fund_id);

		// send message that we're going to confirm
		send_regular_message::<T, I>();

		let relayers_state = UnrewardedRelayersState {
			unrewarded_relayer_entries: 1,
			messages_in_oldest_entry: 1,
			total_messages: 1,
		};
		let proof = T::prepare_message_delivery_proof(MessageDeliveryProofParams {
			lane: T::bench_lane_id(),
			inbound_lane_data: InboundLaneData {
				relayers: vec![(1, 1, relayer_id.clone())].into_iter().collect(),
				last_confirmed_nonce: 0,
			},
			size: ProofSize::Minimal(0),
		});
	}: receive_messages_delivery_proof(RawOrigin::Signed(relayer_id.clone()), proof, relayers_state)
	verify {
		assert_eq!(
			T::account_balance(&relayer_id),
			relayer_balance + MESSAGE_FEE.into(),
		);
	}

	// Benchmark `receive_messages_delivery_proof` extrinsic with following conditions:
	// * single relayer is rewarded for relaying two messages;
	// * relayer account does not exist (in practice it needs to exist in production environment).
	//
	// Additional weight for paying single-message reward to the same relayer could be computed
	// as `weight(receive_delivery_proof_for_two_messages_by_single_relayer)
	//   - weight(receive_delivery_proof_for_single_message)`.
	receive_delivery_proof_for_two_messages_by_single_relayer {
		let relayers_fund_id = crate::Module::<T, I>::relayer_fund_account_id();
		let relayer_id: T::AccountId = account("relayer", 0, SEED);
		let relayer_balance = T::account_balance(&relayer_id);
		T::endow_account(&relayers_fund_id);

		// send message that we're going to confirm
		send_regular_message::<T, I>();
		send_regular_message::<T, I>();

		let relayers_state = UnrewardedRelayersState {
			unrewarded_relayer_entries: 1,
			messages_in_oldest_entry: 2,
			total_messages: 2,
		};
		let proof = T::prepare_message_delivery_proof(MessageDeliveryProofParams {
			lane: T::bench_lane_id(),
			inbound_lane_data: InboundLaneData {
				relayers: vec![(1, 2, relayer_id.clone())].into_iter().collect(),
				last_confirmed_nonce: 0,
			},
			size: ProofSize::Minimal(0),
		});
	}: receive_messages_delivery_proof(RawOrigin::Signed(relayer_id.clone()), proof, relayers_state)
	verify {
		ensure_relayer_rewarded::<T, I>(&relayer_id, &relayer_balance);
	}

	// Benchmark `receive_messages_delivery_proof` extrinsic with following conditions:
	// * two relayers are rewarded for relaying single message each;
	// * relayer account does not exist (in practice it needs to exist in production environment).
	//
	// Additional weight for paying reward to the next relayer could be computed
	// as `weight(receive_delivery_proof_for_two_messages_by_two_relayers)
	//   - weight(receive_delivery_proof_for_two_messages_by_single_relayer)`.
	receive_delivery_proof_for_two_messages_by_two_relayers {
		let relayers_fund_id = crate::Module::<T, I>::relayer_fund_account_id();
		let relayer1_id: T::AccountId = account("relayer1", 1, SEED);
		let relayer1_balance = T::account_balance(&relayer1_id);
		let relayer2_id: T::AccountId = account("relayer2", 2, SEED);
		let relayer2_balance = T::account_balance(&relayer2_id);
		T::endow_account(&relayers_fund_id);

		// send message that we're going to confirm
		send_regular_message::<T, I>();
		send_regular_message::<T, I>();

		let relayers_state = UnrewardedRelayersState {
			unrewarded_relayer_entries: 2,
			messages_in_oldest_entry: 1,
			total_messages: 2,
		};
		let proof = T::prepare_message_delivery_proof(MessageDeliveryProofParams {
			lane: T::bench_lane_id(),
			inbound_lane_data: InboundLaneData {
				relayers: vec![
					(1, 1, relayer1_id.clone()),
					(2, 2, relayer2_id.clone()),
				].into_iter().collect(),
				last_confirmed_nonce: 0,
			},
			size: ProofSize::Minimal(0),
		});
	}: receive_messages_delivery_proof(RawOrigin::Signed(relayer1_id.clone()), proof, relayers_state)
	verify {
		ensure_relayer_rewarded::<T, I>(&relayer1_id, &relayer1_balance);
		ensure_relayer_rewarded::<T, I>(&relayer2_id, &relayer2_balance);
	}

	//
	// Benchmarks for manual checks.
	//

	// Benchmark `send_message` extrinsic with following conditions:
	// * outbound lane already has state, so it needs to be read and decoded;
	// * relayers fund account does not exists (in practice it needs to exist in production environment);
	// * maximal number of messages is being pruned during the call;
	// * message size varies from minimal to maximal for the target chain.
	//
	// Results of this benchmark may be used to check how message size affects `send_message` performance.
	send_messages_of_various_lengths {
		let i in 0..T::maximal_message_size().try_into().unwrap_or_default();

		let lane_id = T::bench_lane_id();
		let sender = account("sender", 0, SEED);
		T::endow_account(&sender);

		// 'send' messages that are to be pruned when our message is sent
		for _nonce in 1..=T::MaxMessagesToPruneAtOnce::get() {
			send_regular_message::<T, I>();
		}
		confirm_message_delivery::<T, I>(T::MaxMessagesToPruneAtOnce::get());

		let (payload, fee) = T::prepare_outbound_message(MessageParams {
			size: i as _,
			sender_account: sender.clone(),
		});
	}: send_message(RawOrigin::Signed(sender), lane_id, payload, fee)
	verify {
		assert_eq!(
			crate::Module::<T, I>::outbound_latest_generated_nonce(T::bench_lane_id()),
			T::MaxMessagesToPruneAtOnce::get() + 1,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with multiple minimal-weight messages and following conditions:
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// This benchmarks gives us an approximation of single message delivery weight. It is similar to the
	// `weight(receive_two_messages_proof) - weight(receive_single_message_proof)`. So it may be used
	// to verify that the other approximation is correct.
	receive_multiple_messages_proof {
		let i in 1..64;

		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);
		let messages_count = i as _;

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=(20 + i as MessageNonce),
			outbound_lane_data: None,
			size: ProofSize::Minimal(EXPECTED_DEFAULT_MESSAGE_LENGTH),
		});
	}: receive_messages_proof(
		RawOrigin::Signed(relayer_id_on_target),
		relayer_id_on_source,
		proof,
		messages_count,
		dispatch_weight
	)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			20 + i as MessageNonce,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with single minimal-weight message and following conditions:
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// Results of this benchmark may be used to check how proof size affects `receive_message_proof` performance.
	receive_message_proofs_with_extra_nodes {
		let i in 0..T::maximal_message_size();

		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);
		let messages_count = 1u32;

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=21,
			outbound_lane_data: None,
			size: ProofSize::HasExtraNodes(i as _),
		});
	}: receive_messages_proof(
		RawOrigin::Signed(relayer_id_on_target),
		relayer_id_on_source,
		proof,
		messages_count,
		dispatch_weight
	)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			21,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with single minimal-weight message and following conditions:
	// * proof does not include outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// Results of this benchmark may be used to check how message size affects `receive_message_proof` performance.
	receive_message_proofs_with_large_leaf {
		let i in 0..T::maximal_message_size();

		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);
		let messages_count = 1u32;

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=21,
			outbound_lane_data: None,
			size: ProofSize::HasLargeLeaf(i as _),
		});
	}: receive_messages_proof(
		RawOrigin::Signed(relayer_id_on_target),
		relayer_id_on_source,
		proof,
		messages_count,
		dispatch_weight
	)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			21,
		);
	}

	// Benchmark `receive_messages_proof` extrinsic with multiple minimal-weight messages and following conditions:
	// * proof includes outbound lane state proof;
	// * inbound lane already has state, so it needs to be read and decoded;
	// * message is successfully dispatched;
	// * message requires all heavy checks done by dispatcher.
	//
	// This benchmarks gives us an approximation of outbound lane state delivery weight. It is similar to the
	// `weight(receive_single_message_proof_with_outbound_lane_state) - weight(receive_single_message_proof)`.
	// So it may be used to verify that the other approximation is correct.
	receive_multiple_messages_proof_with_outbound_lane_state {
		let i in 1..128;

		let relayer_id_on_source = T::bridged_relayer_id();
		let relayer_id_on_target = account("relayer", 0, SEED);
		let messages_count = i as _;

		// mark messages 1..=20 as delivered
		receive_messages::<T, I>(20);

		let (proof, dispatch_weight) = T::prepare_message_proof(MessageProofParams {
			lane: T::bench_lane_id(),
			message_nonces: 21..=20 + i as MessageNonce,
			outbound_lane_data: Some(OutboundLaneData {
				oldest_unpruned_nonce: 21,
				latest_received_nonce: 20,
				latest_generated_nonce: 21,
			}),
			size: ProofSize::Minimal(0),
		});
	}: receive_messages_proof(
		RawOrigin::Signed(relayer_id_on_target),
		relayer_id_on_source,
		proof,
		messages_count,
		dispatch_weight
	)
	verify {
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_received_nonce(T::bench_lane_id()),
			20 + i as MessageNonce,
		);
		assert_eq!(
			crate::Module::<T, I>::inbound_latest_confirmed_nonce(T::bench_lane_id()),
			20,
		);
	}

	// Benchmark `receive_messages_delivery_proof` extrinsic where single relayer delivers multiple messages.
	receive_delivery_proof_for_multiple_messages_by_single_relayer {
		// there actually should be used value of `MaxUnrewardedRelayerEntriesAtInboundLane` from the bridged
		// chain, but we're more interested in additional weight/message than in max weight
		let i in 1..T::MaxUnrewardedRelayerEntriesAtInboundLane::get()
			.try_into()
			.expect("Value of MaxUnrewardedRelayerEntriesAtInboundLane is too large");

		let relayers_fund_id = crate::Module::<T, I>::relayer_fund_account_id();
		let relayer_id: T::AccountId = account("relayer", 0, SEED);
		let relayer_balance = T::account_balance(&relayer_id);
		T::endow_account(&relayers_fund_id);

		// send messages that we're going to confirm
		for _ in 1..=i {
			send_regular_message::<T, I>();
		}

		let relayers_state = UnrewardedRelayersState {
			unrewarded_relayer_entries: 1,
			messages_in_oldest_entry: 1,
			total_messages: i as MessageNonce,
		};
		let proof = T::prepare_message_delivery_proof(MessageDeliveryProofParams {
			lane: T::bench_lane_id(),
			inbound_lane_data: InboundLaneData {
				relayers: vec![(1, i as MessageNonce, relayer_id.clone())].into_iter().collect(),
				last_confirmed_nonce: 0,
			},
			size: ProofSize::Minimal(0),
		});
	}: receive_messages_delivery_proof(RawOrigin::Signed(relayer_id.clone()), proof, relayers_state)
	verify {
		ensure_relayer_rewarded::<T, I>(&relayer_id, &relayer_balance);
	}

	// Benchmark `receive_messages_delivery_proof` extrinsic where every relayer delivers single messages.
	receive_delivery_proof_for_multiple_messages_by_multiple_relayers {
		// there actually should be used value of `MaxUnconfirmedMessagesAtInboundLane` from the bridged
		// chain, but we're more interested in additional weight/message than in max weight
		let i in 1..T::MaxUnconfirmedMessagesAtInboundLane::get()
			.try_into()
			.expect("Value of MaxUnconfirmedMessagesAtInboundLane is too large ");

		let relayers_fund_id = crate::Module::<T, I>::relayer_fund_account_id();
		let confirmation_relayer_id = account("relayer", 0, SEED);
		let relayers: BTreeMap<T::AccountId, T::OutboundMessageFee> = (1..=i)
			.map(|j| {
				let relayer_id = account("relayer", j + 1, SEED);
				let relayer_balance = T::account_balance(&relayer_id);
				(relayer_id, relayer_balance)
			})
			.collect();
		T::endow_account(&relayers_fund_id);

		// send messages that we're going to confirm
		for _ in 1..=i {
			send_regular_message::<T, I>();
		}

		let relayers_state = UnrewardedRelayersState {
			unrewarded_relayer_entries: i as MessageNonce,
			messages_in_oldest_entry: 1,
			total_messages: i as MessageNonce,
		};
		let proof = T::prepare_message_delivery_proof(MessageDeliveryProofParams {
			lane: T::bench_lane_id(),
			inbound_lane_data: InboundLaneData {
				relayers: relayers
					.keys()
					.enumerate()
					.map(|(j, relayer_id)| (j as MessageNonce + 1, j as MessageNonce + 1, relayer_id.clone()))
					.collect(),
				last_confirmed_nonce: 0,
			},
			size: ProofSize::Minimal(0),
		});
	}: receive_messages_delivery_proof(RawOrigin::Signed(confirmation_relayer_id), proof, relayers_state)
	verify {
		for (relayer_id, prev_balance) in relayers {
			ensure_relayer_rewarded::<T, I>(&relayer_id, &prev_balance);
		}
	}
}

fn send_regular_message<T: Config<I>, I: Instance>() {
	let mut outbound_lane = outbound_lane::<T, I>(T::bench_lane_id());
	outbound_lane.send_message(MessageData {
		payload: vec![],
		fee: MESSAGE_FEE.into(),
	});
}

fn send_regular_message_with_payload<T: Config<I>, I: Instance>(payload: Vec<u8>) {
	let mut outbound_lane = outbound_lane::<T, I>(T::bench_lane_id());
	outbound_lane.send_message(MessageData {
		payload,
		fee: MESSAGE_FEE.into(),
	});
}

fn confirm_message_delivery<T: Config<I>, I: Instance>(nonce: MessageNonce) {
	let mut outbound_lane = outbound_lane::<T, I>(T::bench_lane_id());
	assert!(outbound_lane.confirm_delivery(nonce).is_some());
}

fn receive_messages<T: Config<I>, I: Instance>(nonce: MessageNonce) {
	let mut inbound_lane_storage = inbound_lane_storage::<T, I>(T::bench_lane_id());
	inbound_lane_storage.set_data(InboundLaneData {
		relayers: vec![(1, nonce, T::bridged_relayer_id())].into_iter().collect(),
		last_confirmed_nonce: 0,
	});
}

fn ensure_relayer_rewarded<T: Config<I>, I: Instance>(relayer_id: &T::AccountId, old_balance: &T::OutboundMessageFee) {
	let new_balance = T::account_balance(relayer_id);
	assert!(
		new_balance > *old_balance,
		"Relayer haven't received reward for relaying message: old balance = {:?}, new balance = {:?}",
		old_balance,
		new_balance,
	);
}
