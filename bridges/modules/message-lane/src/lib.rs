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

//! Runtime module that allows sending and receiving messages using lane concept:
//!
//! 1) the message is sent using `send_message()` call;
//! 2) every outbound message is assigned nonce;
//! 3) the messages are stored in the storage;
//! 4) external component (relay) delivers messages to bridged chain;
//! 5) messages are processed in order (ordered by assigned nonce);
//! 6) relay may send proof-of-delivery back to this chain.
//!
//! Once message is sent, its progress can be tracked by looking at module events.
//! The assigned nonce is reported using `MessageAccepted` event. When message is
//! delivered to the the bridged chain, it is reported using `MessagesDelivered` event.
//!
//! **IMPORTANT NOTE**: after generating weights (custom `WeighInfo` implementation) for
//! your runtime (where this module is plugged to), please add test for these weights.
//! The test should call the `ensure_weights_are_correct` function from this module.
//! If this test fails with your weights, then either weights are computed incorrectly,
//! or some benchmarks assumptions are broken for your runtime.

#![cfg_attr(not(feature = "std"), no_std)]

pub use crate::weights_ext::{
	ensure_able_to_receive_confirmation, ensure_able_to_receive_message, ensure_weights_are_correct, WeightInfoExt,
	EXPECTED_DEFAULT_MESSAGE_LENGTH,
};

use crate::inbound_lane::{InboundLane, InboundLaneStorage};
use crate::outbound_lane::{OutboundLane, OutboundLaneStorage};
use crate::weights::WeightInfo;

use bp_message_lane::{
	source_chain::{LaneMessageVerifier, MessageDeliveryAndDispatchPayment, RelayersRewards, TargetHeaderChain},
	target_chain::{DispatchMessage, MessageDispatch, ProvedLaneMessages, ProvedMessages, SourceHeaderChain},
	total_unrewarded_messages, InboundLaneData, LaneId, MessageData, MessageKey, MessageNonce, MessagePayload,
	OutboundLaneData, Parameter as MessageLaneParameter, UnrewardedRelayersState,
};
use bp_runtime::Size;
use codec::{Decode, Encode};
use frame_support::{
	decl_error, decl_event, decl_module, decl_storage, ensure,
	traits::Get,
	weights::{DispatchClass, Weight},
	Parameter, StorageMap,
};
use frame_system::{ensure_signed, RawOrigin};
use num_traits::{SaturatingAdd, Zero};
use sp_runtime::{traits::BadOrigin, DispatchResult};
use sp_std::{cell::RefCell, cmp::PartialOrd, marker::PhantomData, prelude::*};

mod inbound_lane;
mod outbound_lane;
mod weights_ext;

pub mod instant_payments;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

#[cfg(test)]
mod mock;

/// The module configuration trait
pub trait Config<I = DefaultInstance>: frame_system::Config {
	// General types

	/// They overarching event type.
	type Event: From<Event<Self, I>> + Into<<Self as frame_system::Config>::Event>;
	/// Benchmarks results from runtime we're plugged into.
	type WeightInfo: WeightInfoExt;
	/// Pallet parameter that is opaque to the pallet itself, but may be used by the runtime
	/// for integrating the pallet.
	///
	/// All pallet parameters may only be updated either by the root, or by the pallet owner.
	type Parameter: MessageLaneParameter;

	/// Maximal number of messages that may be pruned during maintenance. Maintenance occurs
	/// whenever new message is sent. The reason is that if you want to use lane, you should
	/// be ready to pay for its maintenance.
	type MaxMessagesToPruneAtOnce: Get<MessageNonce>;
	/// Maximal number of unrewarded relayer entries at inbound lane. Unrewarded means that the
	/// relayer has delivered messages, but either confirmations haven't been delivered back to the
	/// source chain, or we haven't received reward confirmations yet.
	///
	/// This constant limits maximal number of entries in the `InboundLaneData::relayers`. Keep
	/// in mind that the same relayer account may take several (non-consecutive) entries in this
	/// set.
	type MaxUnrewardedRelayerEntriesAtInboundLane: Get<MessageNonce>;
	/// Maximal number of unconfirmed messages at inbound lane. Unconfirmed means that the
	/// message has been delivered, but either confirmations haven't been delivered back to the
	/// source chain, or we haven't received reward confirmations for these messages yet.
	///
	/// This constant limits difference between last message from last entry of the
	/// `InboundLaneData::relayers` and first message at the first entry.
	///
	/// There is no point of making this parameter lesser than MaxUnrewardedRelayerEntriesAtInboundLane,
	/// because then maximal number of relayer entries will be limited by maximal number of messages.
	///
	/// This value also represents maximal number of messages in single delivery transaction. Transaction
	/// that is declaring more messages than this value, will be rejected. Even if these messages are
	/// from different lanes.
	type MaxUnconfirmedMessagesAtInboundLane: Get<MessageNonce>;

	/// Payload type of outbound messages. This payload is dispatched on the bridged chain.
	type OutboundPayload: Parameter + Size;
	/// Message fee type of outbound messages. This fee is paid on this chain.
	type OutboundMessageFee: Default + From<u64> + PartialOrd + Parameter + SaturatingAdd + Zero;

	/// Payload type of inbound messages. This payload is dispatched on this chain.
	type InboundPayload: Decode;
	/// Message fee type of inbound messages. This fee is paid on the bridged chain.
	type InboundMessageFee: Decode;
	/// Identifier of relayer that deliver messages to this chain. Relayer reward is paid on the bridged chain.
	type InboundRelayer: Parameter;

	/// A type which can be turned into an AccountId from a 256-bit hash.
	///
	/// Used when deriving the shared relayer fund account.
	type AccountIdConverter: sp_runtime::traits::Convert<sp_core::hash::H256, Self::AccountId>;

	// Types that are used by outbound_lane (on source chain).

	/// Target header chain.
	type TargetHeaderChain: TargetHeaderChain<Self::OutboundPayload, Self::AccountId>;
	/// Message payload verifier.
	type LaneMessageVerifier: LaneMessageVerifier<Self::AccountId, Self::OutboundPayload, Self::OutboundMessageFee>;
	/// Message delivery payment.
	type MessageDeliveryAndDispatchPayment: MessageDeliveryAndDispatchPayment<Self::AccountId, Self::OutboundMessageFee>;

	// Types that are used by inbound_lane (on target chain).

	/// Source header chain, as it is represented on target chain.
	type SourceHeaderChain: SourceHeaderChain<Self::InboundMessageFee>;
	/// Message dispatch.
	type MessageDispatch: MessageDispatch<Self::InboundMessageFee, DispatchPayload = Self::InboundPayload>;
}

/// Shortcut to messages proof type for Config.
type MessagesProofOf<T, I> =
	<<T as Config<I>>::SourceHeaderChain as SourceHeaderChain<<T as Config<I>>::InboundMessageFee>>::MessagesProof;
/// Shortcut to messages delivery proof type for Config.
type MessagesDeliveryProofOf<T, I> = <<T as Config<I>>::TargetHeaderChain as TargetHeaderChain<
	<T as Config<I>>::OutboundPayload,
	<T as frame_system::Config>::AccountId,
>>::MessagesDeliveryProof;

decl_error! {
	pub enum Error for Module<T: Config<I>, I: Instance> {
		/// All pallet operations are halted.
		Halted,
		/// Message has been treated as invalid by chain verifier.
		MessageRejectedByChainVerifier,
		/// Message has been treated as invalid by lane verifier.
		MessageRejectedByLaneVerifier,
		/// Submitter has failed to pay fee for delivering and dispatching messages.
		FailedToWithdrawMessageFee,
		/// The transaction brings too many messages.
		TooManyMessagesInTheProof,
		/// Invalid messages has been submitted.
		InvalidMessagesProof,
		/// Invalid messages dispatch weight has been declared by the relayer.
		InvalidMessagesDispatchWeight,
		/// Invalid messages delivery proof has been submitted.
		InvalidMessagesDeliveryProof,
		/// The relayer has declared invalid unrewarded relayers state in the `receive_messages_delivery_proof` call.
		InvalidUnrewardedRelayersState,
		/// The message someone is trying to work with (i.e. increase fee) is already-delivered.
		MessageIsAlreadyDelivered,
		/// The message someone is trying to work with (i.e. increase fee) is not yet sent.
		MessageIsNotYetSent
	}
}

decl_storage! {
	trait Store for Module<T: Config<I>, I: Instance = DefaultInstance> as MessageLane {
		/// Optional pallet owner.
		///
		/// Pallet owner has a right to halt all pallet operations and then resume it. If it is
		/// `None`, then there are no direct ways to halt/resume pallet operations, but other
		/// runtime methods may still be used to do that (i.e. democracy::referendum to update halt
		/// flag directly or call the `halt_operations`).
		pub ModuleOwner get(fn module_owner): Option<T::AccountId>;
		/// If true, all pallet transactions are failed immediately.
		pub IsHalted get(fn is_halted) config(): bool;
		/// Map of lane id => inbound lane data.
		pub InboundLanes: map hasher(blake2_128_concat) LaneId => InboundLaneData<T::InboundRelayer>;
		/// Map of lane id => outbound lane data.
		pub OutboundLanes: map hasher(blake2_128_concat) LaneId => OutboundLaneData;
		/// All queued outbound messages.
		pub OutboundMessages: map hasher(blake2_128_concat) MessageKey => Option<MessageData<T::OutboundMessageFee>>;
	}
	add_extra_genesis {
		config(phantom): sp_std::marker::PhantomData<I>;
		config(owner): Option<T::AccountId>;
		build(|config| {
			if let Some(ref owner) = config.owner {
				<ModuleOwner<T, I>>::put(owner);
			}
		})
	}
}

decl_event!(
	pub enum Event<T, I = DefaultInstance>
	where
		AccountId = <T as frame_system::Config>::AccountId,
		Parameter = <T as Config<I>>::Parameter,
	{
		/// Pallet parameter has been updated.
		ParameterUpdated(Parameter),
		/// Message has been accepted and is waiting to be delivered.
		MessageAccepted(LaneId, MessageNonce),
		/// Messages in the inclusive range have been delivered and processed by the bridged chain.
		MessagesDelivered(LaneId, MessageNonce, MessageNonce),
		/// Phantom member, never used.
		Dummy(PhantomData<(AccountId, I)>),
	}
);

decl_module! {
	pub struct Module<T: Config<I>, I: Instance = DefaultInstance> for enum Call where origin: T::Origin {
		/// Deposit one of this module's events by using the default implementation.
		fn deposit_event() = default;

		/// Ensure runtime invariants.
		fn on_runtime_upgrade() -> Weight {
			let reads = T::MessageDeliveryAndDispatchPayment::initialize(
				&Self::relayer_fund_account_id()
			);
			T::DbWeight::get().reads(reads as u64)
		}

		/// Change `ModuleOwner`.
		///
		/// May only be called either by root, or by `ModuleOwner`.
		#[weight = (T::DbWeight::get().reads_writes(1, 1), DispatchClass::Operational)]
		pub fn set_owner(origin, new_owner: Option<T::AccountId>) {
			ensure_owner_or_root::<T, I>(origin)?;
			match new_owner {
				Some(new_owner) => {
					ModuleOwner::<T, I>::put(&new_owner);
					frame_support::debug::info!("Setting pallet Owner to: {:?}", new_owner);
				},
				None => {
					ModuleOwner::<T, I>::kill();
					frame_support::debug::info!("Removed Owner of pallet.");
				},
			}
		}

		/// Halt all pallet operations. Operations may be resumed using `resume_operations` call.
		///
		/// May only be called either by root, or by `ModuleOwner`.
		#[weight = (T::DbWeight::get().reads_writes(1, 1), DispatchClass::Operational)]
		pub fn halt_operations(origin) {
			ensure_owner_or_root::<T, I>(origin)?;
			IsHalted::<I>::put(true);
			frame_support::debug::warn!("Stopping pallet operations.");
		}

		/// Resume all pallet operations. May be called even if pallet is halted.
		///
		/// May only be called either by root, or by `ModuleOwner`.
		#[weight = (T::DbWeight::get().reads_writes(1, 1), DispatchClass::Operational)]
		pub fn resume_operations(origin) {
			ensure_owner_or_root::<T, I>(origin)?;
			IsHalted::<I>::put(false);
			frame_support::debug::info!("Resuming pallet operations.");
		}

		/// Update pallet parameter.
		///
		/// May only be called either by root, or by `ModuleOwner`.
		///
		/// The weight is: single read for permissions check + 2 writes for parameter value and event.
		#[weight = (T::DbWeight::get().reads_writes(1, 2), DispatchClass::Operational)]
		pub fn update_pallet_parameter(origin, parameter: T::Parameter) {
			ensure_owner_or_root::<T, I>(origin)?;
			parameter.save();
			Self::deposit_event(RawEvent::ParameterUpdated(parameter));
		}

		/// Send message over lane.
		#[weight = T::WeightInfo::send_message_weight(payload)]
		pub fn send_message(
			origin,
			lane_id: LaneId,
			payload: T::OutboundPayload,
			delivery_and_dispatch_fee: T::OutboundMessageFee,
		) -> DispatchResult {
			ensure_operational::<T, I>()?;
			let submitter = origin.into().map_err(|_| BadOrigin)?;

			// let's first check if message can be delivered to target chain
			T::TargetHeaderChain::verify_message(&payload)
				.map_err(|err| {
					frame_support::debug::trace!(
						"Message to lane {:?} is rejected by target chain: {:?}",
						lane_id,
						err,
					);

					Error::<T, I>::MessageRejectedByChainVerifier
				})?;

			// now let's enforce any additional lane rules
			let mut lane = outbound_lane::<T, I>(lane_id);
			T::LaneMessageVerifier::verify_message(
				&submitter,
				&delivery_and_dispatch_fee,
				&lane_id,
				&lane.data(),
				&payload,
			).map_err(|err| {
				frame_support::debug::trace!(
					"Message to lane {:?} is rejected by lane verifier: {:?}",
					lane_id,
					err,
				);

				Error::<T, I>::MessageRejectedByLaneVerifier
			})?;

			// let's withdraw delivery and dispatch fee from submitter
			T::MessageDeliveryAndDispatchPayment::pay_delivery_and_dispatch_fee(
				&submitter,
				&delivery_and_dispatch_fee,
				&Self::relayer_fund_account_id(),
			).map_err(|err| {
				frame_support::debug::trace!(
					"Message to lane {:?} is rejected because submitter {:?} is unable to pay fee {:?}: {:?}",
					lane_id,
					submitter,
					delivery_and_dispatch_fee,
					err,
				);

				Error::<T, I>::FailedToWithdrawMessageFee
			})?;

			// finally, save message in outbound storage and emit event
			let encoded_payload = payload.encode();
			let encoded_payload_len = encoded_payload.len();
			let nonce = lane.send_message(MessageData {
				payload: encoded_payload,
				fee: delivery_and_dispatch_fee,
			});
			lane.prune_messages(T::MaxMessagesToPruneAtOnce::get());

			frame_support::debug::trace!(
				"Accepted message {} to lane {:?}. Message size: {:?}",
				nonce,
				lane_id,
				encoded_payload_len,
			);

			Self::deposit_event(RawEvent::MessageAccepted(lane_id, nonce));

			Ok(())
		}

		/// Pay additional fee for the message.
		#[weight = T::WeightInfo::increase_message_fee()]
		pub fn increase_message_fee(
			origin,
			lane_id: LaneId,
			nonce: MessageNonce,
			additional_fee: T::OutboundMessageFee,
		) -> DispatchResult {
			// if someone tries to pay for already-delivered message, we're rejecting this intention
			// (otherwise this additional fee will be locked forever in relayers fund)
			//
			// if someone tries to pay for not-yet-sent message, we're rejeting this intention, or
			// we're risking to have mess in the storage
			let lane = outbound_lane::<T, I>(lane_id);
			ensure!(nonce > lane.data().latest_received_nonce, Error::<T, I>::MessageIsAlreadyDelivered);
			ensure!(nonce <= lane.data().latest_generated_nonce, Error::<T, I>::MessageIsNotYetSent);

			// withdraw additional fee from submitter
			let submitter = origin.into().map_err(|_| BadOrigin)?;
			T::MessageDeliveryAndDispatchPayment::pay_delivery_and_dispatch_fee(
				&submitter,
				&additional_fee,
				&Self::relayer_fund_account_id(),
			).map_err(|err| {
				frame_support::debug::trace!(
					"Submitter {:?} can't pay additional fee {:?} for the message {:?}/{:?}: {:?}",
					submitter,
					additional_fee,
					lane_id,
					nonce,
					err,
				);

				Error::<T, I>::FailedToWithdrawMessageFee
			})?;

			// and finally update fee in the storage
			let message_key = MessageKey { lane_id, nonce };
			OutboundMessages::<T, I>::mutate(message_key, |message_data| {
				// saturating_add is fine here - overflow here means that someone controls all
				// chain funds, which shouldn't ever happen + `pay_delivery_and_dispatch_fee`
				// above will fail before we reach here
				let message_data = message_data
					.as_mut()
					.expect("the message is sent and not yet delivered; so it is in the storage; qed");
				message_data.fee = message_data.fee.saturating_add(&additional_fee);
			});

			Ok(())
		}

		/// Receive messages proof from bridged chain.
		///
		/// The weight of the call assumes that the transaction always brings outbound lane
		/// state update. Because of that, the submitter (relayer) has no benefit of not including
		/// this data in the transaction, so reward confirmations lags should be minimal.
		#[weight = T::WeightInfo::receive_messages_proof_weight(proof, *messages_count, *dispatch_weight)]
		pub fn receive_messages_proof(
			origin,
			relayer_id: T::InboundRelayer,
			proof: MessagesProofOf<T, I>,
			messages_count: u32,
			dispatch_weight: Weight,
		) -> DispatchResult {
			ensure_operational::<T, I>()?;
			let _ = ensure_signed(origin)?;

			// reject transactions that are declaring too many messages
			ensure!(
				MessageNonce::from(messages_count) <= T::MaxUnconfirmedMessagesAtInboundLane::get(),
				Error::<T, I>::TooManyMessagesInTheProof
			);

			// verify messages proof && convert proof into messages
			let messages = verify_and_decode_messages_proof::<
				T::SourceHeaderChain,
				T::InboundMessageFee,
				T::InboundPayload,
			>(proof, messages_count)
				.map_err(|err| {
					frame_support::debug::trace!(
						"Rejecting invalid messages proof: {:?}",
						err,
					);

					Error::<T, I>::InvalidMessagesProof
				})?;

			// verify that relayer is paying actual dispatch weight
			let actual_dispatch_weight: Weight = messages
				.values()
				.map(|lane_messages| lane_messages
					.messages
					.iter()
					.map(T::MessageDispatch::dispatch_weight)
					.fold(0, |sum, weight| sum.saturating_add(&weight))
				)
				.fold(0, |sum, weight| sum.saturating_add(weight));
			if dispatch_weight < actual_dispatch_weight {
				frame_support::debug::trace!(
					"Rejecting messages proof because of dispatch weight mismatch: declared={}, expected={}",
					dispatch_weight,
					actual_dispatch_weight,
				);

				return Err(Error::<T, I>::InvalidMessagesDispatchWeight.into());
			}

			// dispatch messages and (optionally) update lane(s) state(s)
			let mut total_messages = 0;
			let mut valid_messages = 0;
			for (lane_id, lane_data) in messages {
				let mut lane = inbound_lane::<T, I>(lane_id);

				if let Some(lane_state) = lane_data.lane_state {
					let updated_latest_confirmed_nonce = lane.receive_state_update(lane_state);
					if let Some(updated_latest_confirmed_nonce) = updated_latest_confirmed_nonce {
						frame_support::debug::trace!(
							"Received lane {:?} state update: latest_confirmed_nonce={}",
							lane_id,
							updated_latest_confirmed_nonce,
						);
					}
				}

				for message in lane_data.messages {
					debug_assert_eq!(message.key.lane_id, lane_id);

					total_messages += 1;
					if lane.receive_message::<T::MessageDispatch>(relayer_id.clone(), message.key.nonce, message.data) {
						valid_messages += 1;
					}
				}
			}

			frame_support::debug::trace!(
				"Received messages: total={}, valid={}",
				total_messages,
				valid_messages,
			);

			Ok(())
		}

		/// Receive messages delivery proof from bridged chain.
		#[weight = T::WeightInfo::receive_messages_delivery_proof_weight(proof, relayers_state)]
		pub fn receive_messages_delivery_proof(
			origin,
			proof: MessagesDeliveryProofOf<T, I>,
			relayers_state: UnrewardedRelayersState,
		) -> DispatchResult {
			ensure_operational::<T, I>()?;

			let confirmation_relayer = ensure_signed(origin)?;
			let (lane_id, lane_data) = T::TargetHeaderChain::verify_messages_delivery_proof(proof).map_err(|err| {
				frame_support::debug::trace!(
					"Rejecting invalid messages delivery proof: {:?}",
					err,
				);

				Error::<T, I>::InvalidMessagesDeliveryProof
			})?;

			// verify that the relayer has declared correct `lane_data::relayers` state
			// (we only care about total number of entries and messages, because this affects call weight)
			ensure!(
				total_unrewarded_messages(&lane_data.relayers)
					.unwrap_or(MessageNonce::MAX) == relayers_state.total_messages
					&& lane_data.relayers.len() as MessageNonce == relayers_state.unrewarded_relayer_entries,
				Error::<T, I>::InvalidUnrewardedRelayersState
			);

			// mark messages as delivered
			let mut lane = outbound_lane::<T, I>(lane_id);
			let mut relayers_rewards: RelayersRewards<_, T::OutboundMessageFee> = RelayersRewards::new();
			let last_delivered_nonce = lane_data.last_delivered_nonce();
			let received_range = lane.confirm_delivery(last_delivered_nonce);
			if let Some(received_range) = received_range {
				Self::deposit_event(RawEvent::MessagesDelivered(lane_id, received_range.0, received_range.1));

				// remember to reward relayers that have delivered messages
				// this loop is bounded by `T::MaxUnrewardedRelayerEntriesAtInboundLane` on the bridged chain
				for (nonce_low, nonce_high, relayer) in lane_data.relayers {
					let nonce_begin = sp_std::cmp::max(nonce_low, received_range.0);
					let nonce_end = sp_std::cmp::min(nonce_high, received_range.1);

					// loop won't proceed if current entry is ahead of received range (begin > end).
					// this loop is bound by `T::MaxUnconfirmedMessagesAtInboundLane` on the bridged chain
					let mut relayer_reward = relayers_rewards.entry(relayer).or_default();
					for nonce in nonce_begin..nonce_end + 1 {
						let message_data = OutboundMessages::<T, I>::get(MessageKey {
							lane_id,
							nonce,
						}).expect("message was just confirmed; we never prune unconfirmed messages; qed");
						relayer_reward.reward = relayer_reward.reward.saturating_add(&message_data.fee);
						relayer_reward.messages += 1;
					}
				}
			}

			// if some new messages have been confirmed, reward relayers
			if !relayers_rewards.is_empty() {
				let relayer_fund_account = Self::relayer_fund_account_id();
				<T as Config<I>>::MessageDeliveryAndDispatchPayment::pay_relayers_rewards(
					&confirmation_relayer,
					relayers_rewards,
					&relayer_fund_account,
				);
			}

			frame_support::debug::trace!(
				"Received messages delivery proof up to (and including) {} at lane {:?}",
				last_delivered_nonce,
				lane_id,
			);

			Ok(())
		}
	}
}

impl<T: Config<I>, I: Instance> Module<T, I> {
	/// Get payload of given outbound message.
	pub fn outbound_message_payload(lane: LaneId, nonce: MessageNonce) -> Option<MessagePayload> {
		OutboundMessages::<T, I>::get(MessageKey { lane_id: lane, nonce }).map(|message_data| message_data.payload)
	}

	/// Get nonce of latest generated message at given outbound lane.
	pub fn outbound_latest_generated_nonce(lane: LaneId) -> MessageNonce {
		OutboundLanes::<I>::get(&lane).latest_generated_nonce
	}

	/// Get nonce of latest confirmed message at given outbound lane.
	pub fn outbound_latest_received_nonce(lane: LaneId) -> MessageNonce {
		OutboundLanes::<I>::get(&lane).latest_received_nonce
	}

	/// Get nonce of latest received message at given inbound lane.
	pub fn inbound_latest_received_nonce(lane: LaneId) -> MessageNonce {
		InboundLanes::<T, I>::get(&lane).last_delivered_nonce()
	}

	/// Get nonce of latest confirmed message at given inbound lane.
	pub fn inbound_latest_confirmed_nonce(lane: LaneId) -> MessageNonce {
		InboundLanes::<T, I>::get(&lane).last_confirmed_nonce
	}

	/// Get state of unrewarded relayers set.
	pub fn inbound_unrewarded_relayers_state(
		lane: bp_message_lane::LaneId,
	) -> bp_message_lane::UnrewardedRelayersState {
		let relayers = InboundLanes::<T, I>::get(&lane).relayers;
		bp_message_lane::UnrewardedRelayersState {
			unrewarded_relayer_entries: relayers.len() as _,
			messages_in_oldest_entry: relayers.front().map(|(begin, end, _)| 1 + end - begin).unwrap_or(0),
			total_messages: total_unrewarded_messages(&relayers).unwrap_or(MessageNonce::MAX),
		}
	}

	/// AccountId of the shared relayer fund account.
	///
	/// This account is passed to `MessageDeliveryAndDispatchPayment` trait, and depending
	/// on the implementation it can be used to store relayers rewards.
	/// See [InstantCurrencyPayments] for a concrete implementation.
	pub fn relayer_fund_account_id() -> T::AccountId {
		use sp_runtime::traits::Convert;
		let encoded_id = bp_runtime::derive_relayer_fund_account_id(bp_runtime::NO_INSTANCE_ID);
		T::AccountIdConverter::convert(encoded_id)
	}
}

/// Getting storage keys for messages and lanes states. These keys are normally used when building
/// messages and lanes states proofs.
///
/// Keep in mind that all functions in this module are **NOT** using passed `T` argument, so any
/// runtime can be passed. E.g. if you're verifying proof from Runtime1 in Runtime2, you only have
/// access to Runtime2 and you may pass it to the functions, where required. This is because our
/// maps are not using any Runtime-specific data in the keys.
///
/// On the other side, passing correct instance is required. So if proof has been crafted by the
/// Instance1, you should verify it using Instance1. This is inconvenient if you're using different
/// instances on different sides of the bridge. I.e. in Runtime1 it is Instance2, but on Runtime2
/// it is Instance42. But there's no other way, but to craft this key manually (which is what I'm
/// trying to avoid here) - by using strings like "Instance2", "OutboundMessages", etc.
pub mod storage_keys {
	use super::*;
	use frame_support::storage::generator::StorageMap;
	use sp_core::storage::StorageKey;

	/// Storage key of the outbound message in the runtime storage.
	pub fn message_key<T: Config<I>, I: Instance>(lane: &LaneId, nonce: MessageNonce) -> StorageKey {
		let message_key = MessageKey { lane_id: *lane, nonce };
		let raw_storage_key = OutboundMessages::<T, I>::storage_map_final_key(message_key);
		StorageKey(raw_storage_key)
	}

	/// Storage key of the outbound message lane state in the runtime storage.
	pub fn outbound_lane_data_key<I: Instance>(lane: &LaneId) -> StorageKey {
		StorageKey(OutboundLanes::<I>::storage_map_final_key(*lane))
	}

	/// Storage key of the inbound message lane state in the runtime storage.
	pub fn inbound_lane_data_key<T: Config<I>, I: Instance>(lane: &LaneId) -> StorageKey {
		StorageKey(InboundLanes::<T, I>::storage_map_final_key(*lane))
	}
}

/// Ensure that the origin is either root, or `ModuleOwner`.
fn ensure_owner_or_root<T: Config<I>, I: Instance>(origin: T::Origin) -> Result<(), BadOrigin> {
	match origin.into() {
		Ok(RawOrigin::Root) => Ok(()),
		Ok(RawOrigin::Signed(ref signer)) if Some(signer) == Module::<T, I>::module_owner().as_ref() => Ok(()),
		_ => Err(BadOrigin),
	}
}

/// Ensure that the pallet is in operational mode (not halted).
fn ensure_operational<T: Config<I>, I: Instance>() -> Result<(), Error<T, I>> {
	if IsHalted::<I>::get() {
		Err(Error::<T, I>::Halted)
	} else {
		Ok(())
	}
}

/// Creates new inbound lane object, backed by runtime storage.
fn inbound_lane<T: Config<I>, I: Instance>(lane_id: LaneId) -> InboundLane<RuntimeInboundLaneStorage<T, I>> {
	InboundLane::new(inbound_lane_storage::<T, I>(lane_id))
}

/// Creates new runtime inbound lane storage.
fn inbound_lane_storage<T: Config<I>, I: Instance>(lane_id: LaneId) -> RuntimeInboundLaneStorage<T, I> {
	RuntimeInboundLaneStorage {
		lane_id,
		cached_data: RefCell::new(None),
		_phantom: Default::default(),
	}
}

/// Creates new outbound lane object, backed by runtime storage.
fn outbound_lane<T: Config<I>, I: Instance>(lane_id: LaneId) -> OutboundLane<RuntimeOutboundLaneStorage<T, I>> {
	OutboundLane::new(RuntimeOutboundLaneStorage {
		lane_id,
		_phantom: Default::default(),
	})
}

/// Runtime inbound lane storage.
struct RuntimeInboundLaneStorage<T: Config<I>, I = DefaultInstance> {
	lane_id: LaneId,
	cached_data: RefCell<Option<InboundLaneData<T::InboundRelayer>>>,
	_phantom: PhantomData<I>,
}

impl<T: Config<I>, I: Instance> InboundLaneStorage for RuntimeInboundLaneStorage<T, I> {
	type MessageFee = T::InboundMessageFee;
	type Relayer = T::InboundRelayer;

	fn id(&self) -> LaneId {
		self.lane_id
	}

	fn max_unrewarded_relayer_entries(&self) -> MessageNonce {
		T::MaxUnrewardedRelayerEntriesAtInboundLane::get()
	}

	fn max_unconfirmed_messages(&self) -> MessageNonce {
		T::MaxUnconfirmedMessagesAtInboundLane::get()
	}

	fn data(&self) -> InboundLaneData<T::InboundRelayer> {
		match self.cached_data.clone().into_inner() {
			Some(data) => data,
			None => {
				let data = InboundLanes::<T, I>::get(&self.lane_id);
				*self.cached_data.try_borrow_mut().expect(
					"we're in the single-threaded environment;\
						we have no recursive borrows; qed",
				) = Some(data.clone());
				data
			}
		}
	}

	fn set_data(&mut self, data: InboundLaneData<T::InboundRelayer>) {
		*self.cached_data.try_borrow_mut().expect(
			"we're in the single-threaded environment;\
				we have no recursive borrows; qed",
		) = Some(data.clone());
		InboundLanes::<T, I>::insert(&self.lane_id, data)
	}
}

/// Runtime outbound lane storage.
struct RuntimeOutboundLaneStorage<T, I = DefaultInstance> {
	lane_id: LaneId,
	_phantom: PhantomData<(T, I)>,
}

impl<T: Config<I>, I: Instance> OutboundLaneStorage for RuntimeOutboundLaneStorage<T, I> {
	type MessageFee = T::OutboundMessageFee;

	fn id(&self) -> LaneId {
		self.lane_id
	}

	fn data(&self) -> OutboundLaneData {
		OutboundLanes::<I>::get(&self.lane_id)
	}

	fn set_data(&mut self, data: OutboundLaneData) {
		OutboundLanes::<I>::insert(&self.lane_id, data)
	}

	#[cfg(test)]
	fn message(&self, nonce: &MessageNonce) -> Option<MessageData<T::OutboundMessageFee>> {
		OutboundMessages::<T, I>::get(MessageKey {
			lane_id: self.lane_id,
			nonce: *nonce,
		})
	}

	fn save_message(&mut self, nonce: MessageNonce, mesage_data: MessageData<T::OutboundMessageFee>) {
		OutboundMessages::<T, I>::insert(
			MessageKey {
				lane_id: self.lane_id,
				nonce,
			},
			mesage_data,
		);
	}

	fn remove_message(&mut self, nonce: &MessageNonce) {
		OutboundMessages::<T, I>::remove(MessageKey {
			lane_id: self.lane_id,
			nonce: *nonce,
		});
	}
}

/// Verify messages proof and return proved messages with decoded payload.
fn verify_and_decode_messages_proof<Chain: SourceHeaderChain<Fee>, Fee, DispatchPayload: Decode>(
	proof: Chain::MessagesProof,
	messages_count: u32,
) -> Result<ProvedMessages<DispatchMessage<DispatchPayload, Fee>>, Chain::Error> {
	// `receive_messages_proof` weight formula and `MaxUnconfirmedMessagesAtInboundLane` check
	// guarantees that the `message_count` is sane and Vec<Message> may be allocated.
	// (tx with too many messages will either be rejected from the pool, or will fail earlier)
	Chain::verify_messages_proof(proof, messages_count).map(|messages_by_lane| {
		messages_by_lane
			.into_iter()
			.map(|(lane, lane_data)| {
				(
					lane,
					ProvedLaneMessages {
						lane_state: lane_data.lane_state,
						messages: lane_data.messages.into_iter().map(Into::into).collect(),
					},
				)
			})
			.collect()
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		message, run_test, Event as TestEvent, Origin, TestMessageDeliveryAndDispatchPayment, TestMessageLaneParameter,
		TestMessagesDeliveryProof, TestMessagesProof, TestPayload, TestRuntime, TokenConversionRate,
		PAYLOAD_REJECTED_BY_TARGET_CHAIN, REGULAR_PAYLOAD, TEST_LANE_ID, TEST_RELAYER_A, TEST_RELAYER_B,
	};
	use bp_message_lane::UnrewardedRelayersState;
	use frame_support::{assert_noop, assert_ok};
	use frame_system::{EventRecord, Module as System, Phase};
	use hex_literal::hex;
	use sp_runtime::DispatchError;

	fn get_ready_for_events() {
		System::<TestRuntime>::set_block_number(1);
		System::<TestRuntime>::reset_events();
	}

	fn send_regular_message() {
		get_ready_for_events();

		assert_ok!(Module::<TestRuntime>::send_message(
			Origin::signed(1),
			TEST_LANE_ID,
			REGULAR_PAYLOAD,
			REGULAR_PAYLOAD.1,
		));

		// check event with assigned nonce
		assert_eq!(
			System::<TestRuntime>::events(),
			vec![EventRecord {
				phase: Phase::Initialization,
				event: TestEvent::pallet_message_lane(RawEvent::MessageAccepted(TEST_LANE_ID, 1)),
				topics: vec![],
			}],
		);

		// check that fee has been withdrawn from submitter
		assert!(TestMessageDeliveryAndDispatchPayment::is_fee_paid(1, REGULAR_PAYLOAD.1));
	}

	fn receive_messages_delivery_proof() {
		System::<TestRuntime>::set_block_number(1);
		System::<TestRuntime>::reset_events();

		assert_ok!(Module::<TestRuntime>::receive_messages_delivery_proof(
			Origin::signed(1),
			TestMessagesDeliveryProof(Ok((
				TEST_LANE_ID,
				InboundLaneData {
					last_confirmed_nonce: 1,
					..Default::default()
				},
			))),
			Default::default(),
		));

		assert_eq!(
			System::<TestRuntime>::events(),
			vec![EventRecord {
				phase: Phase::Initialization,
				event: TestEvent::pallet_message_lane(RawEvent::MessagesDelivered(TEST_LANE_ID, 1, 1)),
				topics: vec![],
			}],
		);
	}

	#[test]
	fn pallet_owner_may_change_owner() {
		run_test(|| {
			ModuleOwner::<TestRuntime>::put(2);

			assert_ok!(Module::<TestRuntime>::set_owner(Origin::root(), Some(1)));
			assert_noop!(
				Module::<TestRuntime>::halt_operations(Origin::signed(2)),
				DispatchError::BadOrigin,
			);
			assert_ok!(Module::<TestRuntime>::halt_operations(Origin::root()));

			assert_ok!(Module::<TestRuntime>::set_owner(Origin::signed(1), None));
			assert_noop!(
				Module::<TestRuntime>::resume_operations(Origin::signed(1)),
				DispatchError::BadOrigin,
			);
			assert_noop!(
				Module::<TestRuntime>::resume_operations(Origin::signed(2)),
				DispatchError::BadOrigin,
			);
			assert_ok!(Module::<TestRuntime>::resume_operations(Origin::root()));
		});
	}

	#[test]
	fn pallet_may_be_halted_by_root() {
		run_test(|| {
			assert_ok!(Module::<TestRuntime>::halt_operations(Origin::root()));
			assert_ok!(Module::<TestRuntime>::resume_operations(Origin::root()));
		});
	}

	#[test]
	fn pallet_may_be_halted_by_owner() {
		run_test(|| {
			ModuleOwner::<TestRuntime>::put(2);

			assert_ok!(Module::<TestRuntime>::halt_operations(Origin::signed(2)));
			assert_ok!(Module::<TestRuntime>::resume_operations(Origin::signed(2)));

			assert_noop!(
				Module::<TestRuntime>::halt_operations(Origin::signed(1)),
				DispatchError::BadOrigin,
			);
			assert_noop!(
				Module::<TestRuntime>::resume_operations(Origin::signed(1)),
				DispatchError::BadOrigin,
			);

			assert_ok!(Module::<TestRuntime>::halt_operations(Origin::signed(2)));
			assert_noop!(
				Module::<TestRuntime>::resume_operations(Origin::signed(1)),
				DispatchError::BadOrigin,
			);
		});
	}

	#[test]
	fn pallet_parameter_may_be_updated_by_root() {
		run_test(|| {
			get_ready_for_events();

			let parameter = TestMessageLaneParameter::TokenConversionRate(10.into());
			assert_ok!(Module::<TestRuntime>::update_pallet_parameter(
				Origin::root(),
				parameter.clone(),
			));

			assert_eq!(TokenConversionRate::get(), 10.into());
			assert_eq!(
				System::<TestRuntime>::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: TestEvent::pallet_message_lane(RawEvent::ParameterUpdated(parameter)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn pallet_parameter_may_be_updated_by_owner() {
		run_test(|| {
			ModuleOwner::<TestRuntime>::put(2);
			get_ready_for_events();

			let parameter = TestMessageLaneParameter::TokenConversionRate(10.into());
			assert_ok!(Module::<TestRuntime>::update_pallet_parameter(
				Origin::signed(2),
				parameter.clone(),
			));

			assert_eq!(TokenConversionRate::get(), 10.into());
			assert_eq!(
				System::<TestRuntime>::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: TestEvent::pallet_message_lane(RawEvent::ParameterUpdated(parameter)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn pallet_parameter_cant_be_updated_by_arbitrary_submitter() {
		run_test(|| {
			assert_noop!(
				Module::<TestRuntime>::update_pallet_parameter(
					Origin::signed(2),
					TestMessageLaneParameter::TokenConversionRate(10.into()),
				),
				DispatchError::BadOrigin,
			);

			ModuleOwner::<TestRuntime>::put(2);

			assert_noop!(
				Module::<TestRuntime>::update_pallet_parameter(
					Origin::signed(1),
					TestMessageLaneParameter::TokenConversionRate(10.into()),
				),
				DispatchError::BadOrigin,
			);
		});
	}

	#[test]
	fn fixed_u128_works_as_i_think() {
		// this test is here just to be sure that conversion rate may be represented with FixedU128
		run_test(|| {
			use sp_runtime::{FixedPointNumber, FixedU128};

			// 1:1 conversion that we use by default for testnets
			let rialto_token = 1u64;
			let rialto_token_in_millau_tokens = TokenConversionRate::get().saturating_mul_int(rialto_token);
			assert_eq!(rialto_token_in_millau_tokens, 1);

			// let's say conversion rate is 1:1.7
			let conversion_rate = FixedU128::saturating_from_rational(170, 100);
			let rialto_tokens = 100u64;
			let rialto_tokens_in_millau_tokens = conversion_rate.saturating_mul_int(rialto_tokens);
			assert_eq!(rialto_tokens_in_millau_tokens, 170);

			// let's say conversion rate is 1:0.25
			let conversion_rate = FixedU128::saturating_from_rational(25, 100);
			let rialto_tokens = 100u64;
			let rialto_tokens_in_millau_tokens = conversion_rate.saturating_mul_int(rialto_tokens);
			assert_eq!(rialto_tokens_in_millau_tokens, 25);
		});
	}

	#[test]
	fn pallet_rejects_transactions_if_halted() {
		run_test(|| {
			// send message first to be able to check that delivery_proof fails later
			send_regular_message();

			IsHalted::<DefaultInstance>::put(true);

			assert_noop!(
				Module::<TestRuntime>::send_message(
					Origin::signed(1),
					TEST_LANE_ID,
					REGULAR_PAYLOAD,
					REGULAR_PAYLOAD.1,
				),
				Error::<TestRuntime, DefaultInstance>::Halted,
			);

			assert_noop!(
				Module::<TestRuntime>::receive_messages_proof(
					Origin::signed(1),
					TEST_RELAYER_A,
					Ok(vec![message(2, REGULAR_PAYLOAD)]).into(),
					1,
					REGULAR_PAYLOAD.1,
				),
				Error::<TestRuntime, DefaultInstance>::Halted,
			);

			assert_noop!(
				Module::<TestRuntime>::receive_messages_delivery_proof(
					Origin::signed(1),
					TestMessagesDeliveryProof(Ok((
						TEST_LANE_ID,
						InboundLaneData {
							last_confirmed_nonce: 1,
							..Default::default()
						},
					))),
					Default::default(),
				),
				Error::<TestRuntime, DefaultInstance>::Halted,
			);
		});
	}

	#[test]
	fn send_message_works() {
		run_test(|| {
			send_regular_message();
		});
	}

	#[test]
	fn chain_verifier_rejects_invalid_message_in_send_message() {
		run_test(|| {
			// messages with this payload are rejected by target chain verifier
			assert_noop!(
				Module::<TestRuntime>::send_message(
					Origin::signed(1),
					TEST_LANE_ID,
					PAYLOAD_REJECTED_BY_TARGET_CHAIN,
					PAYLOAD_REJECTED_BY_TARGET_CHAIN.1
				),
				Error::<TestRuntime, DefaultInstance>::MessageRejectedByChainVerifier,
			);
		});
	}

	#[test]
	fn lane_verifier_rejects_invalid_message_in_send_message() {
		run_test(|| {
			// messages with zero fee are rejected by lane verifier
			assert_noop!(
				Module::<TestRuntime>::send_message(Origin::signed(1), TEST_LANE_ID, REGULAR_PAYLOAD, 0),
				Error::<TestRuntime, DefaultInstance>::MessageRejectedByLaneVerifier,
			);
		});
	}

	#[test]
	fn message_send_fails_if_submitter_cant_pay_message_fee() {
		run_test(|| {
			TestMessageDeliveryAndDispatchPayment::reject_payments();
			assert_noop!(
				Module::<TestRuntime>::send_message(
					Origin::signed(1),
					TEST_LANE_ID,
					REGULAR_PAYLOAD,
					REGULAR_PAYLOAD.1
				),
				Error::<TestRuntime, DefaultInstance>::FailedToWithdrawMessageFee,
			);
		});
	}

	#[test]
	fn receive_messages_proof_works() {
		run_test(|| {
			assert_ok!(Module::<TestRuntime>::receive_messages_proof(
				Origin::signed(1),
				TEST_RELAYER_A,
				Ok(vec![message(1, REGULAR_PAYLOAD)]).into(),
				1,
				REGULAR_PAYLOAD.1,
			));

			assert_eq!(InboundLanes::<TestRuntime>::get(TEST_LANE_ID).last_delivered_nonce(), 1);
		});
	}

	#[test]
	fn receive_messages_proof_updates_confirmed_message_nonce() {
		run_test(|| {
			// say we have received 10 messages && last confirmed message is 8
			InboundLanes::<TestRuntime, DefaultInstance>::insert(
				TEST_LANE_ID,
				InboundLaneData {
					last_confirmed_nonce: 8,
					relayers: vec![(9, 9, TEST_RELAYER_A), (10, 10, TEST_RELAYER_B)]
						.into_iter()
						.collect(),
				},
			);
			assert_eq!(
				Module::<TestRuntime>::inbound_unrewarded_relayers_state(TEST_LANE_ID),
				UnrewardedRelayersState {
					unrewarded_relayer_entries: 2,
					messages_in_oldest_entry: 1,
					total_messages: 2,
				},
			);

			// message proof includes outbound lane state with latest confirmed message updated to 9
			let mut message_proof: TestMessagesProof = Ok(vec![message(11, REGULAR_PAYLOAD)]).into();
			message_proof.result.as_mut().unwrap()[0].1.lane_state = Some(OutboundLaneData {
				latest_received_nonce: 9,
				..Default::default()
			});

			assert_ok!(Module::<TestRuntime>::receive_messages_proof(
				Origin::signed(1),
				TEST_RELAYER_A,
				message_proof,
				1,
				REGULAR_PAYLOAD.1,
			));

			assert_eq!(
				InboundLanes::<TestRuntime>::get(TEST_LANE_ID),
				InboundLaneData {
					last_confirmed_nonce: 9,
					relayers: vec![(10, 10, TEST_RELAYER_B), (11, 11, TEST_RELAYER_A)]
						.into_iter()
						.collect(),
				},
			);
			assert_eq!(
				Module::<TestRuntime>::inbound_unrewarded_relayers_state(TEST_LANE_ID),
				UnrewardedRelayersState {
					unrewarded_relayer_entries: 2,
					messages_in_oldest_entry: 1,
					total_messages: 2,
				},
			);
		});
	}

	#[test]
	fn receive_messages_proof_rejects_invalid_dispatch_weight() {
		run_test(|| {
			assert_noop!(
				Module::<TestRuntime>::receive_messages_proof(
					Origin::signed(1),
					TEST_RELAYER_A,
					Ok(vec![message(1, REGULAR_PAYLOAD)]).into(),
					1,
					REGULAR_PAYLOAD.1 - 1,
				),
				Error::<TestRuntime, DefaultInstance>::InvalidMessagesDispatchWeight,
			);
		});
	}

	#[test]
	fn receive_messages_proof_rejects_invalid_proof() {
		run_test(|| {
			assert_noop!(
				Module::<TestRuntime, DefaultInstance>::receive_messages_proof(
					Origin::signed(1),
					TEST_RELAYER_A,
					Err(()).into(),
					1,
					0,
				),
				Error::<TestRuntime, DefaultInstance>::InvalidMessagesProof,
			);
		});
	}

	#[test]
	fn receive_messages_proof_rejects_proof_with_too_many_messages() {
		run_test(|| {
			assert_noop!(
				Module::<TestRuntime, DefaultInstance>::receive_messages_proof(
					Origin::signed(1),
					TEST_RELAYER_A,
					Ok(vec![message(1, REGULAR_PAYLOAD)]).into(),
					u32::MAX,
					0,
				),
				Error::<TestRuntime, DefaultInstance>::TooManyMessagesInTheProof,
			);
		});
	}

	#[test]
	fn receive_messages_delivery_proof_works() {
		run_test(|| {
			send_regular_message();
			receive_messages_delivery_proof();

			assert_eq!(
				OutboundLanes::<DefaultInstance>::get(&TEST_LANE_ID).latest_received_nonce,
				1,
			);
		});
	}

	#[test]
	fn receive_messages_delivery_proof_rewards_relayers() {
		run_test(|| {
			assert_ok!(Module::<TestRuntime>::send_message(
				Origin::signed(1),
				TEST_LANE_ID,
				REGULAR_PAYLOAD,
				1000,
			));
			assert_ok!(Module::<TestRuntime>::send_message(
				Origin::signed(1),
				TEST_LANE_ID,
				REGULAR_PAYLOAD,
				2000,
			));

			// this reports delivery of message 1 => reward is paid to TEST_RELAYER_A
			assert_ok!(Module::<TestRuntime>::receive_messages_delivery_proof(
				Origin::signed(1),
				TestMessagesDeliveryProof(Ok((
					TEST_LANE_ID,
					InboundLaneData {
						relayers: vec![(1, 1, TEST_RELAYER_A)].into_iter().collect(),
						..Default::default()
					}
				))),
				UnrewardedRelayersState {
					unrewarded_relayer_entries: 1,
					total_messages: 1,
					..Default::default()
				},
			));
			assert!(TestMessageDeliveryAndDispatchPayment::is_reward_paid(
				TEST_RELAYER_A,
				1000
			));
			assert!(!TestMessageDeliveryAndDispatchPayment::is_reward_paid(
				TEST_RELAYER_B,
				2000
			));

			// this reports delivery of both message 1 and message 2 => reward is paid only to TEST_RELAYER_B
			assert_ok!(Module::<TestRuntime>::receive_messages_delivery_proof(
				Origin::signed(1),
				TestMessagesDeliveryProof(Ok((
					TEST_LANE_ID,
					InboundLaneData {
						relayers: vec![(1, 1, TEST_RELAYER_A), (2, 2, TEST_RELAYER_B)]
							.into_iter()
							.collect(),
						..Default::default()
					}
				))),
				UnrewardedRelayersState {
					unrewarded_relayer_entries: 2,
					total_messages: 2,
					..Default::default()
				},
			));
			assert!(!TestMessageDeliveryAndDispatchPayment::is_reward_paid(
				TEST_RELAYER_A,
				1000
			));
			assert!(TestMessageDeliveryAndDispatchPayment::is_reward_paid(
				TEST_RELAYER_B,
				2000
			));
		});
	}

	#[test]
	fn receive_messages_delivery_proof_rejects_invalid_proof() {
		run_test(|| {
			assert_noop!(
				Module::<TestRuntime>::receive_messages_delivery_proof(
					Origin::signed(1),
					TestMessagesDeliveryProof(Err(())),
					Default::default(),
				),
				Error::<TestRuntime, DefaultInstance>::InvalidMessagesDeliveryProof,
			);
		});
	}

	#[test]
	fn receive_messages_delivery_proof_rejects_proof_if_declared_relayers_state_is_invalid() {
		run_test(|| {
			// when number of relayers entires is invalid
			assert_noop!(
				Module::<TestRuntime>::receive_messages_delivery_proof(
					Origin::signed(1),
					TestMessagesDeliveryProof(Ok((
						TEST_LANE_ID,
						InboundLaneData {
							relayers: vec![(1, 1, TEST_RELAYER_A), (2, 2, TEST_RELAYER_B)]
								.into_iter()
								.collect(),
							..Default::default()
						}
					))),
					UnrewardedRelayersState {
						unrewarded_relayer_entries: 1,
						total_messages: 2,
						..Default::default()
					},
				),
				Error::<TestRuntime, DefaultInstance>::InvalidUnrewardedRelayersState,
			);

			// when number of messages is invalid
			assert_noop!(
				Module::<TestRuntime>::receive_messages_delivery_proof(
					Origin::signed(1),
					TestMessagesDeliveryProof(Ok((
						TEST_LANE_ID,
						InboundLaneData {
							relayers: vec![(1, 1, TEST_RELAYER_A), (2, 2, TEST_RELAYER_B)]
								.into_iter()
								.collect(),
							..Default::default()
						}
					))),
					UnrewardedRelayersState {
						unrewarded_relayer_entries: 2,
						total_messages: 1,
						..Default::default()
					},
				),
				Error::<TestRuntime, DefaultInstance>::InvalidUnrewardedRelayersState,
			);
		});
	}

	#[test]
	fn receive_messages_accepts_single_message_with_invalid_payload() {
		run_test(|| {
			let mut invalid_message = message(1, REGULAR_PAYLOAD);
			invalid_message.data.payload = Vec::new();

			assert_ok!(Module::<TestRuntime, DefaultInstance>::receive_messages_proof(
				Origin::signed(1),
				TEST_RELAYER_A,
				Ok(vec![invalid_message]).into(),
				1,
				0, // weight may be zero in this case (all messages are improperly encoded)
			),);

			assert_eq!(
				InboundLanes::<TestRuntime>::get(&TEST_LANE_ID).last_delivered_nonce(),
				1,
			);
		});
	}

	#[test]
	fn receive_messages_accepts_batch_with_message_with_invalid_payload() {
		run_test(|| {
			let mut invalid_message = message(2, REGULAR_PAYLOAD);
			invalid_message.data.payload = Vec::new();

			assert_ok!(Module::<TestRuntime, DefaultInstance>::receive_messages_proof(
				Origin::signed(1),
				TEST_RELAYER_A,
				Ok(vec![
					message(1, REGULAR_PAYLOAD),
					invalid_message,
					message(3, REGULAR_PAYLOAD),
				])
				.into(),
				3,
				REGULAR_PAYLOAD.1 + REGULAR_PAYLOAD.1,
			),);

			assert_eq!(
				InboundLanes::<TestRuntime>::get(&TEST_LANE_ID).last_delivered_nonce(),
				3,
			);
		});
	}

	#[test]
	fn storage_message_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is breaking all
		// previously crafted messages proofs.
		assert_eq!(
			storage_keys::message_key::<TestRuntime, DefaultInstance>(&*b"test", 42).0,
			hex!("87f1ffe31b52878f09495ca7482df1a48a395e6242c6813b196ca31ed0547ea79446af0e09063bd4a7874aef8a997cec746573742a00000000000000").to_vec(),
		);
	}

	#[test]
	fn outbound_lane_data_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is breaking all
		// previously crafted outbound lane state proofs.
		assert_eq!(
			storage_keys::outbound_lane_data_key::<DefaultInstance>(&*b"test").0,
			hex!("87f1ffe31b52878f09495ca7482df1a496c246acb9b55077390e3ca723a0ca1f44a8995dd50b6657a037a7839304535b74657374").to_vec(),
		);
	}

	#[test]
	fn inbound_lane_data_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is breaking all
		// previously crafted inbound lane state proofs.
		assert_eq!(
			storage_keys::inbound_lane_data_key::<TestRuntime, DefaultInstance>(&*b"test").0,
			hex!("87f1ffe31b52878f09495ca7482df1a4e5f83cf83f2127eb47afdc35d6e43fab44a8995dd50b6657a037a7839304535b74657374").to_vec(),
		);
	}

	#[test]
	fn actual_dispatch_weight_does_not_overlow() {
		run_test(|| {
			let message1 = message(1, TestPayload(0, Weight::MAX / 2));
			let message2 = message(2, TestPayload(0, Weight::MAX / 2));
			let message3 = message(2, TestPayload(0, Weight::MAX / 2));

			assert_noop!(
				Module::<TestRuntime, DefaultInstance>::receive_messages_proof(
					Origin::signed(1),
					TEST_RELAYER_A,
					// this may cause overflow if source chain storage is invalid
					Ok(vec![message1, message2, message3]).into(),
					3,
					100,
				),
				Error::<TestRuntime, DefaultInstance>::InvalidMessagesDispatchWeight,
			);
		});
	}

	#[test]
	fn increase_message_fee_fails_if_message_is_already_delivered() {
		run_test(|| {
			send_regular_message();
			receive_messages_delivery_proof();

			assert_noop!(
				Module::<TestRuntime, DefaultInstance>::increase_message_fee(Origin::signed(1), TEST_LANE_ID, 1, 100,),
				Error::<TestRuntime, DefaultInstance>::MessageIsAlreadyDelivered,
			);
		});
	}

	#[test]
	fn increase_message_fee_fails_if_message_is_not_yet_sent() {
		run_test(|| {
			assert_noop!(
				Module::<TestRuntime, DefaultInstance>::increase_message_fee(Origin::signed(1), TEST_LANE_ID, 1, 100,),
				Error::<TestRuntime, DefaultInstance>::MessageIsNotYetSent,
			);
		});
	}

	#[test]
	fn increase_message_fee_fails_if_submitter_cant_pay_additional_fee() {
		run_test(|| {
			send_regular_message();

			TestMessageDeliveryAndDispatchPayment::reject_payments();

			assert_noop!(
				Module::<TestRuntime, DefaultInstance>::increase_message_fee(Origin::signed(1), TEST_LANE_ID, 1, 100,),
				Error::<TestRuntime, DefaultInstance>::FailedToWithdrawMessageFee,
			);
		});
	}

	#[test]
	fn increase_message_fee_succeeds() {
		run_test(|| {
			send_regular_message();

			assert_ok!(Module::<TestRuntime, DefaultInstance>::increase_message_fee(
				Origin::signed(1),
				TEST_LANE_ID,
				1,
				100,
			),);
			assert!(TestMessageDeliveryAndDispatchPayment::is_fee_paid(1, 100));
		});
	}
}
