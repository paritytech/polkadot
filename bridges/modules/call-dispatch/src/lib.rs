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

//! Runtime module which takes care of dispatching messages received over the bridge.
//!
//! The messages are interpreted directly as runtime `Call`. We attempt to decode
//! them and then dispatch as usual. To prevent compatibility issues, the Calls have
//! to include a `spec_version`. This will be checked before dispatch. In the case of
//! a succesful dispatch an event is emitted.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]

use bp_message_dispatch::{MessageDispatch, Weight};
use bp_runtime::{derive_account_id, InstanceId, Size, SourceAccount};
use codec::{Decode, Encode};
use frame_support::{
	decl_event, decl_module, decl_storage,
	dispatch::{Dispatchable, Parameter},
	ensure,
	traits::{Filter, Get},
	weights::{extract_actual_weight, GetDispatchInfo},
	RuntimeDebug,
};
use frame_system::RawOrigin;
use sp_runtime::{
	traits::{BadOrigin, Convert, IdentifyAccount, MaybeDisplay, MaybeSerializeDeserialize, Member, Verify},
	DispatchResult,
};
use sp_std::{fmt::Debug, marker::PhantomData, prelude::*};

/// Spec version type.
pub type SpecVersion = u32;

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

/// Message payload type used by call-dispatch module.
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

/// The module configuration trait.
pub trait Config<I = DefaultInstance>: frame_system::Config {
	/// The overarching event type.
	type Event: From<Event<Self, I>> + Into<<Self as frame_system::Config>::Event>;
	/// Id of the message. Whenever message is passed to the dispatch module, it emits
	/// event with this id + dispatch result. Could be e.g. (LaneId, MessageNonce) if
	/// it comes from message-lane module.
	type MessageId: Parameter;
	/// Type of account ID on source chain.
	type SourceChainAccountId: Parameter + Member + MaybeSerializeDeserialize + Debug + MaybeDisplay + Ord + Default;
	/// Type of account public key on target chain.
	type TargetChainAccountPublic: Parameter + IdentifyAccount<AccountId = Self::AccountId>;
	/// Type of signature that may prove that the message has been signed by
	/// owner of `TargetChainAccountPublic`.
	type TargetChainSignature: Parameter + Verify<Signer = Self::TargetChainAccountPublic>;
	/// The overarching dispatch call type.
	type Call: Parameter
		+ GetDispatchInfo
		+ Dispatchable<
			Origin = <Self as frame_system::Config>::Origin,
			PostInfo = frame_support::dispatch::PostDispatchInfo,
		>;
	/// Pre-dispatch filter for incoming calls.
	///
	/// The pallet will filter all incoming calls right before they're dispatched. If this filter
	/// rejects the call, special event (`Event::MessageCallRejected`) is emitted.
	type CallFilter: Filter<<Self as Config<I>>::Call>;
	/// The type that is used to wrap the `Self::Call` when it is moved over bridge.
	///
	/// The idea behind this is to avoid `Call` conversion/decoding until we'll be sure
	/// that all other stuff (like `spec_version`) is ok. If we would try to decode
	/// `Call` which has been encoded using previous `spec_version`, then we might end
	/// up with decoding error, instead of `MessageVersionSpecMismatch`.
	type EncodedCall: Decode + Encode + Into<Result<<Self as Config<I>>::Call, ()>>;
	/// A type which can be turned into an AccountId from a 256-bit hash.
	///
	/// Used when deriving target chain AccountIds from source chain AccountIds.
	type AccountIdConverter: sp_runtime::traits::Convert<sp_core::hash::H256, Self::AccountId>;
}

decl_storage! {
	trait Store for Module<T: Config<I>, I: Instance = DefaultInstance> as CallDispatch {}
}

decl_event!(
	pub enum Event<T, I = DefaultInstance> where
		<T as Config<I>>::MessageId
	{
		/// Message has been rejected before reaching dispatch.
		MessageRejected(InstanceId, MessageId),
		/// Message has been rejected by dispatcher because of spec version mismatch.
		/// Last two arguments are: expected and passed spec version.
		MessageVersionSpecMismatch(InstanceId, MessageId, SpecVersion, SpecVersion),
		/// Message has been rejected by dispatcher because of weight mismatch.
		/// Last two arguments are: expected and passed call weight.
		MessageWeightMismatch(InstanceId, MessageId, Weight, Weight),
		/// Message signature mismatch.
		MessageSignatureMismatch(InstanceId, MessageId),
		/// Message has been dispatched with given result.
		MessageDispatched(InstanceId, MessageId, DispatchResult),
		/// We have failed to decode Call from the message.
		MessageCallDecodeFailed(InstanceId, MessageId),
		/// The call from the message has been rejected by the call filter.
		MessageCallRejected(InstanceId, MessageId),
		/// Phantom member, never used. Needed to handle multiple pallet instances.
		_Dummy(PhantomData<I>),
	}
);

decl_module! {
	/// Call Dispatch FRAME Pallet.
	pub struct Module<T: Config<I>, I: Instance = DefaultInstance> for enum Call where origin: T::Origin {
		/// Deposit one of this module's events by using the default implementation.
		fn deposit_event() = default;
	}
}

impl<T: Config<I>, I: Instance> MessageDispatch<T::MessageId> for Module<T, I> {
	type Message =
		MessagePayload<T::SourceChainAccountId, T::TargetChainAccountPublic, T::TargetChainSignature, T::EncodedCall>;

	fn dispatch_weight(message: &Self::Message) -> Weight {
		message.weight
	}

	fn dispatch(bridge: InstanceId, id: T::MessageId, message: Result<Self::Message, ()>) {
		// emit special even if message has been rejected by external component
		let message = match message {
			Ok(message) => message,
			Err(_) => {
				frame_support::debug::trace!("Message {:?}/{:?}: rejected before actual dispatch", bridge, id);
				Self::deposit_event(RawEvent::MessageRejected(bridge, id));
				return;
			}
		};

		// verify spec version
		// (we want it to be the same, because otherwise we may decode Call improperly)
		let expected_version = <T as frame_system::Config>::Version::get().spec_version;
		if message.spec_version != expected_version {
			frame_support::debug::trace!(
				"Message {:?}/{:?}: spec_version mismatch. Expected {:?}, got {:?}",
				bridge,
				id,
				expected_version,
				message.spec_version,
			);
			Self::deposit_event(RawEvent::MessageVersionSpecMismatch(
				bridge,
				id,
				expected_version,
				message.spec_version,
			));
			return;
		}

		// now that we have spec version checked, let's decode the call
		let call = match message.call.into() {
			Ok(call) => call,
			Err(_) => {
				frame_support::debug::trace!("Failed to decode Call from message {:?}/{:?}", bridge, id,);
				Self::deposit_event(RawEvent::MessageCallDecodeFailed(bridge, id));
				return;
			}
		};

		// prepare dispatch origin
		let origin_account = match message.origin {
			CallOrigin::SourceRoot => {
				let hex_id = derive_account_id::<T::SourceChainAccountId>(bridge, SourceAccount::Root);
				let target_id = T::AccountIdConverter::convert(hex_id);
				frame_support::debug::trace!("Root Account: {:?}", &target_id);
				target_id
			}
			CallOrigin::TargetAccount(source_account_id, target_public, target_signature) => {
				let digest = account_ownership_digest(&call, source_account_id, message.spec_version, bridge);

				let target_account = target_public.into_account();
				if !target_signature.verify(&digest[..], &target_account) {
					frame_support::debug::trace!(
						"Message {:?}/{:?}: origin proof is invalid. Expected account: {:?} from signature: {:?}",
						bridge,
						id,
						target_account,
						target_signature,
					);
					Self::deposit_event(RawEvent::MessageSignatureMismatch(bridge, id));
					return;
				}

				frame_support::debug::trace!("Target Account: {:?}", &target_account);
				target_account
			}
			CallOrigin::SourceAccount(source_account_id) => {
				let hex_id = derive_account_id(bridge, SourceAccount::Account(source_account_id));
				let target_id = T::AccountIdConverter::convert(hex_id);
				frame_support::debug::trace!("Source Account: {:?}", &target_id);
				target_id
			}
		};

		// filter the call
		if !T::CallFilter::filter(&call) {
			frame_support::debug::trace!(
				"Message {:?}/{:?}: the call ({:?}) is rejected by filter",
				bridge,
				id,
				call,
			);
			Self::deposit_event(RawEvent::MessageCallRejected(bridge, id));
			return;
		}

		// verify weight
		// (we want passed weight to be at least equal to pre-dispatch weight of the call
		// because otherwise Calls may be dispatched at lower price)
		let dispatch_info = call.get_dispatch_info();
		let expected_weight = dispatch_info.weight;
		if message.weight < expected_weight {
			frame_support::debug::trace!(
				"Message {:?}/{:?}: passed weight is too low. Expected at least {:?}, got {:?}",
				bridge,
				id,
				expected_weight,
				message.weight,
			);
			Self::deposit_event(RawEvent::MessageWeightMismatch(
				bridge,
				id,
				expected_weight,
				message.weight,
			));
			return;
		}

		// finally dispatch message
		let origin = RawOrigin::Signed(origin_account).into();

		frame_support::debug::trace!("Message being dispatched is: {:?}", &call);
		let dispatch_result = call.dispatch(origin);
		let actual_call_weight = extract_actual_weight(&dispatch_result, &dispatch_info);

		frame_support::debug::trace!(
			"Message {:?}/{:?} has been dispatched. Weight: {} of {}. Result: {:?}",
			bridge,
			id,
			actual_call_weight,
			message.weight,
			dispatch_result,
		);

		Self::deposit_event(RawEvent::MessageDispatched(
			bridge,
			id,
			dispatch_result.map(drop).map_err(|e| e.error),
		));
	}
}

/// Check if the message is allowed to be dispatched on the target chain given the sender's origin
/// on the source chain.
///
/// For example, if a message is sent from a "regular" account on the source chain it will not be
/// allowed to be dispatched as Root on the target chain. This is a useful check to do on the source
/// chain _before_ sending a message whose dispatch will be rejected on the target chain.
pub fn verify_message_origin<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature, Call>(
	sender_origin: &RawOrigin<SourceChainAccountId>,
	message: &MessagePayload<SourceChainAccountId, TargetChainAccountPublic, TargetChainSignature, Call>,
) -> Result<Option<SourceChainAccountId>, BadOrigin>
where
	SourceChainAccountId: PartialEq + Clone,
{
	match message.origin {
		CallOrigin::SourceRoot => {
			ensure!(sender_origin == &RawOrigin::Root, BadOrigin);
			Ok(None)
		}
		CallOrigin::TargetAccount(ref source_account_id, _, _) => {
			ensure!(
				sender_origin == &RawOrigin::Signed(source_account_id.clone()),
				BadOrigin
			);
			Ok(Some(source_account_id.clone()))
		}
		CallOrigin::SourceAccount(ref source_account_id) => {
			ensure!(
				sender_origin == &RawOrigin::Signed(source_account_id.clone()),
				BadOrigin
			);
			Ok(Some(source_account_id.clone()))
		}
	}
}

/// Target account ownership digest from the source chain.
///
/// The byte vector returned by this function will be signed with a target chain account
/// private key. This way, the owner of `source_account_id` on the source chain proves that
/// the target chain account private key is also under his control.
pub fn account_ownership_digest<Call, AccountId, SpecVersion, BridgeId>(
	call: &Call,
	source_account_id: AccountId,
	target_spec_version: SpecVersion,
	source_instance_id: BridgeId,
) -> Vec<u8>
where
	Call: Encode,
	AccountId: Encode,
	SpecVersion: Encode,
	BridgeId: Encode,
{
	let mut proof = Vec::new();
	call.encode_to(&mut proof);
	source_account_id.encode_to(&mut proof);
	target_spec_version.encode_to(&mut proof);
	source_instance_id.encode_to(&mut proof);

	proof
}

#[cfg(test)]
mod tests {
	// From construct_runtime macro
	#![allow(clippy::from_over_into)]

	use super::*;
	use frame_support::{parameter_types, weights::Weight};
	use frame_system::{EventRecord, Phase};
	use sp_core::H256;
	use sp_runtime::{
		testing::Header,
		traits::{BlakeTwo256, IdentityLookup},
		Perbill,
	};

	type AccountId = u64;
	type MessageId = [u8; 4];

	#[derive(Debug, Encode, Decode, Clone, PartialEq, Eq)]
	pub struct TestAccountPublic(AccountId);

	impl IdentifyAccount for TestAccountPublic {
		type AccountId = AccountId;

		fn into_account(self) -> AccountId {
			self.0
		}
	}

	#[derive(Debug, Encode, Decode, Clone, PartialEq, Eq)]
	pub struct TestSignature(AccountId);

	impl Verify for TestSignature {
		type Signer = TestAccountPublic;

		fn verify<L: sp_runtime::traits::Lazy<[u8]>>(&self, _msg: L, signer: &AccountId) -> bool {
			self.0 == *signer
		}
	}

	pub struct AccountIdConverter;

	impl sp_runtime::traits::Convert<H256, AccountId> for AccountIdConverter {
		fn convert(hash: H256) -> AccountId {
			hash.to_low_u64_ne()
		}
	}

	type Block = frame_system::mocking::MockBlock<TestRuntime>;
	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<TestRuntime>;

	use crate as call_dispatch;

	frame_support::construct_runtime! {
		pub enum TestRuntime where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Module, Call, Config, Storage, Event<T>},
			CallDispatch: call_dispatch::{Module, Call, Event<T>},
		}
	}

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}

	impl frame_system::Config for TestRuntime {
		type Origin = Origin;
		type Index = u64;
		type Call = Call;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = AccountId;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = ();
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type BaseCallFilter = ();
		type SystemWeightInfo = ();
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type SS58Prefix = ();
	}

	impl Config for TestRuntime {
		type Event = Event;
		type MessageId = MessageId;
		type SourceChainAccountId = AccountId;
		type TargetChainAccountPublic = TestAccountPublic;
		type TargetChainSignature = TestSignature;
		type Call = Call;
		type CallFilter = TestCallFilter;
		type EncodedCall = EncodedCall;
		type AccountIdConverter = AccountIdConverter;
	}

	#[derive(Decode, Encode)]
	pub struct EncodedCall(Vec<u8>);

	impl From<EncodedCall> for Result<Call, ()> {
		fn from(call: EncodedCall) -> Result<Call, ()> {
			Call::decode(&mut &call.0[..]).map_err(drop)
		}
	}

	pub struct TestCallFilter;

	impl Filter<Call> for TestCallFilter {
		fn filter(call: &Call) -> bool {
			!matches!(*call, Call::System(frame_system::Call::fill_block(_)))
		}
	}

	const TEST_SPEC_VERSION: SpecVersion = 0;
	const TEST_WEIGHT: Weight = 1_000_000_000;

	fn new_test_ext() -> sp_io::TestExternalities {
		let t = frame_system::GenesisConfig::default()
			.build_storage::<TestRuntime>()
			.unwrap();
		sp_io::TestExternalities::new(t)
	}

	fn prepare_message(
		origin: CallOrigin<AccountId, TestAccountPublic, TestSignature>,
		call: Call,
	) -> <Module<TestRuntime> as MessageDispatch<<TestRuntime as Config>::MessageId>>::Message {
		MessagePayload {
			spec_version: TEST_SPEC_VERSION,
			weight: TEST_WEIGHT,
			origin,
			call: EncodedCall(call.encode()),
		}
	}

	fn prepare_root_message(
		call: Call,
	) -> <Module<TestRuntime> as MessageDispatch<<TestRuntime as Config>::MessageId>>::Message {
		prepare_message(CallOrigin::SourceRoot, call)
	}

	fn prepare_target_message(
		call: Call,
	) -> <Module<TestRuntime> as MessageDispatch<<TestRuntime as Config>::MessageId>>::Message {
		let origin = CallOrigin::TargetAccount(1, TestAccountPublic(1), TestSignature(1));
		prepare_message(origin, call)
	}

	fn prepare_source_message(
		call: Call,
	) -> <Module<TestRuntime> as MessageDispatch<<TestRuntime as Config>::MessageId>>::Message {
		let origin = CallOrigin::SourceAccount(1);
		prepare_message(origin, call)
	}

	#[test]
	fn should_fail_on_spec_version_mismatch() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];

			const BAD_SPEC_VERSION: SpecVersion = 99;
			let mut message =
				prepare_root_message(Call::System(<frame_system::Call<TestRuntime>>::remark(vec![1, 2, 3])));
			message.spec_version = BAD_SPEC_VERSION;

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageVersionSpecMismatch(
						bridge,
						id,
						TEST_SPEC_VERSION,
						BAD_SPEC_VERSION
					)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_fail_on_weight_mismatch() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];
			let mut message =
				prepare_root_message(Call::System(<frame_system::Call<TestRuntime>>::remark(vec![1, 2, 3])));
			message.weight = 0;

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageWeightMismatch(
						bridge, id, 1973000, 0,
					)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_fail_on_signature_mismatch() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];

			let call_origin = CallOrigin::TargetAccount(1, TestAccountPublic(1), TestSignature(99));
			let message = prepare_message(
				call_origin,
				Call::System(<frame_system::Call<TestRuntime>>::remark(vec![1, 2, 3])),
			);

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageSignatureMismatch(
						bridge, id
					)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_emit_event_for_rejected_messages() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Err(()));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageRejected(bridge, id)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_fail_on_call_decode() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];

			let mut message =
				prepare_root_message(Call::System(<frame_system::Call<TestRuntime>>::remark(vec![1, 2, 3])));
			message.call.0 = vec![];

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageCallDecodeFailed(
						bridge, id
					)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_emit_event_for_rejected_calls() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];

			let call = Call::System(<frame_system::Call<TestRuntime>>::fill_block(Perbill::from_percent(75)));
			let weight = call.get_dispatch_info().weight;
			let mut message = prepare_root_message(call);
			message.weight = weight;

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageCallRejected(bridge, id)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_dispatch_bridge_message_from_root_origin() {
		new_test_ext().execute_with(|| {
			let bridge = b"ethb".to_owned();
			let id = [0; 4];
			let message = prepare_root_message(Call::System(<frame_system::Call<TestRuntime>>::remark(vec![1, 2, 3])));

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageDispatched(
						bridge,
						id,
						Ok(())
					)),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_dispatch_bridge_message_from_target_origin() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let bridge = b"ethb".to_owned();

			let call = Call::System(<frame_system::Call<TestRuntime>>::remark(vec![]));
			let message = prepare_target_message(call);

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageDispatched(
						bridge,
						id,
						Ok(())
					)),
					topics: vec![],
				}],
			);
		})
	}

	#[test]
	fn should_dispatch_bridge_message_from_source_origin() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let bridge = b"ethb".to_owned();

			let call = Call::System(<frame_system::Call<TestRuntime>>::remark(vec![]));
			let message = prepare_source_message(call);

			System::set_block_number(1);
			CallDispatch::dispatch(bridge, id, Ok(message));

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: Event::call_dispatch(call_dispatch::Event::<TestRuntime>::MessageDispatched(
						bridge,
						id,
						Ok(())
					)),
					topics: vec![],
				}],
			);
		})
	}

	#[test]
	fn origin_is_checked_when_verifying_sending_message_using_source_root_account() {
		let call = Call::System(<frame_system::Call<TestRuntime>>::remark(vec![]));
		let message = prepare_root_message(call);

		// When message is sent by Root, CallOrigin::SourceRoot is allowed
		assert!(matches!(verify_message_origin(&RawOrigin::Root, &message), Ok(None)));

		// when message is sent by some real account, CallOrigin::SourceRoot is not allowed
		assert!(matches!(
			verify_message_origin(&RawOrigin::Signed(1), &message),
			Err(BadOrigin)
		));
	}

	#[test]
	fn origin_is_checked_when_verifying_sending_message_using_target_account() {
		let call = Call::System(<frame_system::Call<TestRuntime>>::remark(vec![]));
		let message = prepare_target_message(call);

		// When message is sent by Root, CallOrigin::TargetAccount is not allowed
		assert!(matches!(
			verify_message_origin(&RawOrigin::Root, &message),
			Err(BadOrigin)
		));

		// When message is sent by some other account, it is rejected
		assert!(matches!(
			verify_message_origin(&RawOrigin::Signed(2), &message),
			Err(BadOrigin)
		));

		// When message is sent by a real account, it is allowed to have origin
		// CallOrigin::TargetAccount
		assert!(matches!(
			verify_message_origin(&RawOrigin::Signed(1), &message),
			Ok(Some(1))
		));
	}

	#[test]
	fn origin_is_checked_when_verifying_sending_message_using_source_account() {
		let call = Call::System(<frame_system::Call<TestRuntime>>::remark(vec![]));
		let message = prepare_source_message(call);

		// Sending a message from the expected origin account works
		assert!(matches!(
			verify_message_origin(&RawOrigin::Signed(1), &message),
			Ok(Some(1))
		));

		// If we send a message from a different account, it is rejected
		assert!(matches!(
			verify_message_origin(&RawOrigin::Signed(2), &message),
			Err(BadOrigin)
		));

		// If we try and send the message from Root, it is also rejected
		assert!(matches!(
			verify_message_origin(&RawOrigin::Root, &message),
			Err(BadOrigin)
		));
	}
}
