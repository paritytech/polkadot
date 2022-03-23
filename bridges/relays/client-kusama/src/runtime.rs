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

//! Types that are specific to the Kusama runtime.

use bp_messages::{LaneId, UnrewardedRelayersState};
use bp_polkadot_core::{AccountAddress, Balance, PolkadotLike};
use bp_runtime::Chain;
use codec::{Compact, Decode, Encode};
use frame_support::weights::Weight;
use scale_info::TypeInfo;
use sp_runtime::FixedU128;

/// Unchecked Kusama extrinsic.
pub type UncheckedExtrinsic = bp_polkadot_core::UncheckedExtrinsic<Call>;

/// Polkadot account ownership digest from Kusama.
///
/// The byte vector returned by this function should be signed with a Polkadot account private key.
/// This way, the owner of `kusama_account_id` on Kusama proves that the Polkadot account private
/// key is also under his control.
pub fn kusama_to_polkadot_account_ownership_digest<Call, AccountId, SpecVersion>(
	polkadot_call: &Call,
	kusama_account_id: AccountId,
	polkadot_spec_version: SpecVersion,
) -> Vec<u8>
where
	Call: codec::Encode,
	AccountId: codec::Encode,
	SpecVersion: codec::Encode,
{
	pallet_bridge_dispatch::account_ownership_digest(
		polkadot_call,
		kusama_account_id,
		polkadot_spec_version,
		bp_runtime::KUSAMA_CHAIN_ID,
		bp_runtime::POLKADOT_CHAIN_ID,
	)
}

/// Kusama Runtime `Call` enum.
///
/// The enum represents a subset of possible `Call`s we can send to Kusama chain.
/// Ideally this code would be auto-generated from metadata, because we want to
/// avoid depending directly on the ENTIRE runtime just to get the encoding of `Dispatchable`s.
///
/// All entries here (like pretty much in the entire file) must be kept in sync with Kusama
/// `construct_runtime`, so that we maintain SCALE-compatibility.
///
/// See: [link](https://github.com/paritytech/polkadot/blob/master/runtime/kusama/src/lib.rs)
#[allow(clippy::large_enum_variant)]
#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
pub enum Call {
	/// System pallet.
	#[codec(index = 0)]
	System(SystemCall),
	/// Balances pallet.
	#[codec(index = 4)]
	Balances(BalancesCall),
	/// Utility pallet.
	#[codec(index = 24)]
	Utility(UtilityCall),
	/// Polkadot bridge pallet.
	#[codec(index = 110)]
	BridgePolkadotGrandpa(BridgePolkadotGrandpaCall),
	/// Polkadot messages pallet.
	#[codec(index = 111)]
	BridgePolkadotMessages(BridgePolkadotMessagesCall),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
#[allow(non_camel_case_types)]
pub enum SystemCall {
	#[codec(index = 1)]
	remark(Vec<u8>),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
#[allow(non_camel_case_types)]
pub enum BalancesCall {
	#[codec(index = 0)]
	transfer(AccountAddress, Compact<Balance>),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
#[allow(non_camel_case_types)]
pub enum BridgePolkadotGrandpaCall {
	#[codec(index = 0)]
	submit_finality_proof(
		Box<<PolkadotLike as Chain>::Header>,
		bp_header_chain::justification::GrandpaJustification<<PolkadotLike as Chain>::Header>,
	),
	#[codec(index = 1)]
	initialize(bp_header_chain::InitializationData<<PolkadotLike as Chain>::Header>),
	#[codec(index = 3)]
	set_operational(bool),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
#[allow(non_camel_case_types)]
pub enum BridgePolkadotMessagesCall {
	#[codec(index = 2)]
	update_pallet_parameter(BridgePolkadotMessagesParameter),
	#[codec(index = 3)]
	send_message(
		LaneId,
		bp_message_dispatch::MessagePayload<
			bp_kusama::AccountId,
			bp_polkadot::AccountId,
			bp_polkadot::AccountPublic,
			Vec<u8>,
		>,
		bp_kusama::Balance,
	),
	#[codec(index = 5)]
	receive_messages_proof(
		bp_polkadot::AccountId,
		bridge_runtime_common::messages::target::FromBridgedChainMessagesProof<bp_polkadot::Hash>,
		u32,
		Weight,
	),
	#[codec(index = 6)]
	receive_messages_delivery_proof(
		bridge_runtime_common::messages::source::FromBridgedChainMessagesDeliveryProof<
			bp_polkadot::Hash,
		>,
		UnrewardedRelayersState,
	),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
#[allow(non_camel_case_types)]
pub enum UtilityCall {
	#[codec(index = 2)]
	batch_all(Vec<Call>),
}

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
pub enum BridgePolkadotMessagesParameter {
	#[codec(index = 0)]
	PolkadotToKusamaConversionRate(FixedU128),
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
