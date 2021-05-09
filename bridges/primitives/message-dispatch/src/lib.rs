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

//! A common interface for all Bridge Message Dispatch modules.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]

use bp_runtime::{InstanceId, Size};
use codec::{Decode, Encode};
use frame_support::RuntimeDebug;
use sp_std::prelude::*;

/// Message dispatch weight.
pub type Weight = u64;

/// Spec version type.
pub type SpecVersion = u32;

/// A generic trait to dispatch arbitrary messages delivered over the bridge.
pub trait MessageDispatch<MessageId> {
	/// A type of the message to be dispatched.
	type Message: codec::Decode;

	/// Estimate dispatch weight.
	///
	/// This function must: (1) be instant and (2) return correct upper bound
	/// of dispatch weight.
	fn dispatch_weight(message: &Self::Message) -> Weight;

	/// Dispatches the message internally.
	///
	/// `bridge` indicates instance of deployed bridge where the message came from.
	///
	/// `id` is a short unique identifier of the message.
	///
	/// If message is `Ok`, then it should be dispatched. If it is `Err`, then it's just
	/// a sign that some other component has rejected the message even before it has
	/// reached `dispatch` method (right now this may only be caused if we fail to decode
	/// the whole message).
	fn dispatch(bridge: InstanceId, id: MessageId, message: Result<Self::Message, ()>);
}

/// Origin of a Call when it is dispatched on the target chain.
///
/// The source chain can (and should) verify that the message can be dispatched on the target chain
/// with a particular origin given the source chain's origin. This can be done with the
/// `verify_message_origin()` function.
#[derive(RuntimeDebug, Encode, Decode, Clone, PartialEq, Eq)]
pub enum CallOrigin<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature> {
	/// Call is sent by the Root origin on the source chain. On the target chain it is dispatched
	/// from a derived account.
	///
	/// The derived account represents the source Root account on the target chain. This is useful
	/// if the target chain needs some way of knowing that a call came from a priviledged origin on
	/// the source chain (maybe to allow a configuration change for example).
	SourceRoot,

	/// Call is sent by `SourceChainAccountId` on the source chain. On the target chain it is
	/// dispatched from an account controlled by a private key on the target chain.
	///
	/// The account can be identified by `TargetChainAccountPublic`. The proof that the
	/// `SourceChainAccountId` controls `TargetChainAccountPublic` is the `TargetChainSignature`
	/// over `(Call, SourceChainAccountId, TargetChainSpecVersion, SourceChainBridgeId).encode()`.
	///
	/// NOTE sending messages using this origin (or any other) does not have replay protection!
	/// The assumption is that both the source account and the target account is controlled by
	/// the same entity, so source-chain replay protection is sufficient.
	/// As a consequence, it's extremely important for the target chain user to never produce
	/// a signature with their target-private key on something that could be sent over the bridge,
	/// i.e. if the target user signs `(<some-source-account-id>, Call::Transfer(X, 5))`
	/// The owner of `some-source-account-id` can send that message multiple times, which would
	/// result with multiple transfer calls being dispatched on the target chain.
	/// So please, NEVER USE YOUR PRIVATE KEY TO SIGN SOMETHING YOU DON'T FULLY UNDERSTAND!
	TargetAccount(SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature),

	/// Call is sent by the `SourceChainAccountId` on the source chain. On the target chain it is
	/// dispatched from a derived account ID.
	///
	/// The account ID on the target chain is derived from the source account ID This is useful if
	/// you need a way to represent foreign accounts on this chain for call dispatch purposes.
	///
	/// Note that the derived account does not need to have a private key on the target chain. This
	/// origin can therefore represent proxies, pallets, etc. as well as "regular" accounts.
	SourceAccount(SourceChainAccountId),
}

/// Message payload type used by dispatch module.
#[derive(RuntimeDebug, Encode, Decode, Clone, PartialEq, Eq)]
pub struct MessagePayload<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature, Call> {
	/// Runtime specification version. We only dispatch messages that have the same
	/// runtime version. Otherwise we risk to misinterpret encoded calls.
	pub spec_version: SpecVersion,
	/// Weight of the call, declared by the message sender. If it is less than actual
	/// static weight, the call is not dispatched.
	pub weight: Weight,
	/// Call origin to be used during dispatch.
	pub origin: CallOrigin<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature>,
	/// The call itself.
	pub call: Call,
}

impl<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature> Size
	for MessagePayload<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature, Vec<u8>>
{
	fn size_hint(&self) -> u32 {
		self.call.len() as _
	}
}
