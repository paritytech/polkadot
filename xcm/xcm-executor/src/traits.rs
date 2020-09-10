// Copyright 2020 Parity Technologies (UK) Ltd.
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

use sp_std::{
	result::Result, convert::TryFrom,
	collections::{btree_map::{BTreeMap, Entry}, btree_set::BTreeSet},
	marker::PhantomData,
};
use sp_core::crypto::UncheckedFrom;
use sp_runtime::traits::CheckedConversion;
use xcm::v0::{XcmError, XcmResult, MultiAsset, MultiLocation, MultiNetwork, Junction, MultiOrigin};
use frame_support::traits::Get;
use frame_system::Origin;

pub trait TransactAsset {
	/// Deposit the `what` asset into the account of `who`.
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult;

	/// Withdraw the given asset from the consensus system. Return the actual asset withdrawn. In
	/// the case of `what` being a wildcard, this may be something more specific.
	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<MultiAsset, XcmError>;
}

impl<X: TransactAsset, Y: TransactAsset> TransactAsset for (X, Y) {
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult {
		X::deposit_asset(what, who).or_else(|| Y::deposit_asset(what, who))
	}
	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<MultiAsset, XcmError> {
		X::withdraw_asset(what, who).or_else(|| Y::withdraw_asset(what, who))
	}
}

pub trait MatchesFungible<Balance> {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance>;
}
pub struct IsConcrete<T>(PhantomData<T>);
impl<T: Get<MultiLocation>, B: From<u128>> MatchesFungible<B> for IsConcrete<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match a {
			MultiAsset::ConcreteFungible { id, amount } if id == T::get() =>
				amount.checked_into(),
			_ => false,
		}
	}
}
pub struct IsAbstract<T>(PhantomData<T>);
impl<T: Get<&'static [u8]>, B: From<u128>> MatchesFungible<B> for IsAbstract<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match a {
			MultiAsset::AbstractFungible { id, amount } if &id[..] == T::get() =>
				amount.checked_into(),
			_ => false,
		}
	}
}
impl<B: From<u128>, X: MatchesFungible<B>, Y: MatchesFungible<B>> MatchesFungible<B> for (X, Y) {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		X::matches_fungible(a).or_else(|| Y::matches_fungible(a))
	}
}

pub trait PunnFromLocation<T> {
	fn punn_from_location(m: &MultiLocation) -> Option<T>;
}

pub struct AccountId32Punner<AccountId, Network>(PhantomData<AccountId>, PhantomData<Network>);
impl<AccountId: UncheckedFrom<[u8; 32]>, Network> PunnFromLocation<AccountId> for AccountId32Punner<AccountId, Network> {
	fn punn_from_location(m: &MultiLocation) -> Option<AccountId> {
		match m {
			MultiLocation::X1(Junction::AccountId32 { ref id, .. }) =>
				Some(AccountId::unchecked_from(id.clone())),
			_ => None,
		}
	}
}

pub trait PunnIntoLocation<T> {
	fn punn_into_location(m: T) -> Option<MultiLocation>;
}

impl<
	AccountId: Into<[u8; 32]>,
	Network: Get<MultiNetwork>
> PunnIntoLocation<AccountId> for AccountId32Punner<AccountId, Network> {
	fn punn_into_location(m: AccountId) -> Option<MultiLocation> {
		Some(MultiLocation::X1(Junction::AccountId32 { id: m.into(), network: Network::get() }))
	}
}

pub trait ConvertOrigin<Origin> {
	fn convert_origin(origin: MultiLocation, kind: MultiOrigin) -> Result<Origin, XcmError>;
}

impl<T: frame_system::Trait> ConvertOrigin<T> for () {
	fn convert_origin(_: MultiLocation, _: MultiOrigin) -> Result<Origin<T>, XcmError> { Err(()) }
}

// TODO: This might need placing in polkadot and/or cumulus primitives so the relevant modules can dispatch with native
//    origins.
/*
use polkadot_parachain::primitives::AccountIdConversion;

/// Origin for the parachains module.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
pub enum Origin {
	/// It comes from the relay-chain.
	RelayChain,
	/// It comes from a parachain.
	Parachain(ParaId),
}

// TODO: Implement OriginConverter with this code with generic Relay/Parachain origin.
let origin: <T as Trait>::Origin = match origin_type {
	// TODO: Allow sovereign accounts to be configured via the trait.
	MultiOrigin::SovereignAccount => {
		match origin {
			// Relay-chain doesn't yet have a sovereign account on the parachain.
			MultiLocation::X1(Junction::Parent) => Err(())?,
			MultiLocation::X2(Junction::Parent, Junction::Parachain(id)) =>
				RawOrigin::Signed(id.into_account()).into(),
			_ => Err(())?,
		}
	}
	// We assume we are a parachain.
	//
	// TODO: Use the config trait to convert the multilocation into an origin.
	MultiOrigin::Native => match origin {
		MultiLocation::X1(Junction::Parent) => Origin::RelayChain.into(),
		MultiLocation::X2(Junction::Parent, Junction::Parachain(id)) =>
			Origin::Parachain(id.into()).into(),
		_ => Err(())?,
	},
	MultiOrigin::Superuser => match origin {
		MultiLocation::X1(Junction::Parent) =>
			// We assume that the relay-chain is allowed to execute with superuser
			// privileges if it wants.
			// TODO: allow this to be configurable in the trait.
			RawOrigin::Root.into(),
		MultiLocation::X2(Junction::Parent, Junction::Parachain(id)) =>
			// We assume that parachains are not allowed to execute with
			// superuser privileges.
			// TODO: allow this to be configurable in the trait.
			Err(())?,
		_ => Err(())?,
	}
};
*/
