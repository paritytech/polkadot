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

//! Types that are specific to the Wococo runtime.

use bp_messages::{LaneId, UnrewardedRelayersState};
use bp_polkadot_core::PolkadotLike;
use bp_runtime::Chain;
use codec::{Decode, Encode};
use frame_support::weights::Weight;

/// Instance of messages pallet that is used to bridge with Rococo chain.
pub type WithRococoMessagesInstance = pallet_bridge_messages::DefaultInstance;

/// Unchecked Wococo extrinsic.
pub type UncheckedExtrinsic = bp_polkadot_core::UncheckedExtrinsic<Call>;

/// Rococo account ownership digest from Wococo.
///
/// The byte vector returned by this function should be signed with a Rococo account private key.
/// This way, the owner of `wococo_account_id` on Rococo proves that the Rococo account private key
/// is also under his control.
pub fn wococo_to_rococo_account_ownership_digest<Call, AccountId, SpecVersion>(
	rococo_call: &Call,
	wococo_account_id: AccountId,
	rococo_spec_version: SpecVersion,
) -> Vec<u8>
where
	Call: codec::Encode,
	AccountId: codec::Encode,
	SpecVersion: codec::Encode,
{
	pallet_bridge_dispatch::account_ownership_digest(
		rococo_call,
		wococo_account_id,
		rococo_spec_version,
		bp_runtime::WOCOCO_CHAIN_ID,
		bp_runtime::ROCOCO_CHAIN_ID,
	)
}

/// Wococo Runtime `Call` enum.
///
/// The enum represents a subset of possible `Call`s we can send to Rococo chain.
/// Ideally this code would be auto-generated from Metadata, because we want to
/// avoid depending directly on the ENTIRE runtime just to get the encoding of `Dispatchable`s.
///
/// All entries here (like pretty much in the entire file) must be kept in sync with Rococo
/// `construct_runtime`, so that we maintain SCALE-compatibility.
///
/// See: https://github.com/paritytech/polkadot/blob/master/runtime/rococo/src/lib.rs
#[allow(clippy::large_enum_variant)]
#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
pub enum Call {
	/// System pallet.
	#[codec(index = 0)]
	System(SystemCall),
	/// Rococo bridge pallet.
	#[codec(index = 40)]
	BridgeGrandpaRococo(BridgeGrandpaRococoCall),
	/// Rococo messages pallet.
	#[codec(index = 43)]
	BridgeMessagesRococo(BridgeMessagesRococoCall),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
#[allow(non_camel_case_types)]
pub enum SystemCall {
	#[codec(index = 1)]
	remark(Vec<u8>),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
#[allow(non_camel_case_types)]
pub enum BridgeGrandpaRococoCall {
	#[codec(index = 0)]
	submit_finality_proof(
		<PolkadotLike as Chain>::Header,
		bp_header_chain::justification::GrandpaJustification<<PolkadotLike as Chain>::Header>,
	),
	#[codec(index = 1)]
	initialize(bp_header_chain::InitializationData<<PolkadotLike as Chain>::Header>),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
#[allow(non_camel_case_types)]
pub enum BridgeMessagesRococoCall {
	#[codec(index = 3)]
	send_message(
		LaneId,
		bp_message_dispatch::MessagePayload<
			bp_rococo::AccountId,
			bp_wococo::AccountId,
			bp_wococo::AccountPublic,
			Vec<u8>,
		>,
		bp_rococo::Balance,
	),
	#[codec(index = 5)]
	receive_messages_proof(
		bp_rococo::AccountId,
		bridge_runtime_common::messages::target::FromBridgedChainMessagesProof<bp_rococo::Hash>,
		u32,
		Weight,
	),
	#[codec(index = 6)]
	receive_messages_delivery_proof(
		bridge_runtime_common::messages::source::FromBridgedChainMessagesDeliveryProof<bp_rococo::Hash>,
		UnrewardedRelayersState,
	),
}

impl sp_runtime::traits::Dispatchable for Call {
	type Origin = ();
	type Config = ();
	type Info = ();
	type PostInfo = ();

	fn dispatch(self, _origin: Self::Origin) -> sp_runtime::DispatchResultWithInfo<Self::PostInfo> {
		unimplemented!("The Call is not expected to be dispatched.")
	}
}
