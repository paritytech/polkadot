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

use sp_std::marker::PhantomData;
use frame_support::traits::{Get, OriginTrait};
use xcm::v0::{MultiLocation, OriginKind, NetworkId, Junction};
use xcm_executor::traits::{LocationConversion, ConvertOrigin};
use polkadot_parachain::primitives::IsSystem;

/// Sovereign accounts use the system's `Signed` origin with an account ID derived from the
/// `LocationConverter`.
pub struct SovereignSignedViaLocation<LocationConverter, Origin>(
	PhantomData<(LocationConverter, Origin)>
);
impl<
	LocationConverter: LocationConversion<Origin::AccountId>,
	Origin: OriginTrait,
> ConvertOrigin<Origin> for SovereignSignedViaLocation<LocationConverter, Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		if let OriginKind::SovereignAccount = kind {
			let location = LocationConverter::from_location(&origin).ok_or(origin)?;
			Ok(Origin::signed(location).into())
		} else {
			Err(origin)
		}
	}
}

pub struct ParentAsSuperuser<Origin>(PhantomData<Origin>);
impl<
	Origin: OriginTrait,
> ConvertOrigin<Origin> for ParentAsSuperuser<Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Superuser, MultiLocation::X1(Junction::Parent)) =>
				Ok(Origin::root()),
			(_, origin) => Err(origin),
		}
	}
}

pub struct ChildSystemParachainAsSuperuser<ParaId, Origin>(PhantomData<(ParaId, Origin)>);
impl<
	ParaId: IsSystem + From<u32>,
	Origin: OriginTrait,
> ConvertOrigin<Origin> for ChildSystemParachainAsSuperuser<ParaId, Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Superuser, MultiLocation::X1(Junction::Parachain { id }))
			if ParaId::from(id).is_system() =>
				Ok(Origin::root()),
			(_, origin) => Err(origin),
		}
	}
}

pub struct SiblingSystemParachainAsSuperuser<ParaId, Origin>(PhantomData<(ParaId, Origin)>);
impl<
	ParaId: IsSystem + From<u32>,
	Origin: OriginTrait
> ConvertOrigin<Origin> for SiblingSystemParachainAsSuperuser<ParaId, Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Superuser, MultiLocation::X2(Junction::Parent, Junction::Parachain { id }))
			if ParaId::from(id).is_system() =>
				Ok(Origin::root()),
			(_, origin) => Err(origin),
		}
	}
}

pub struct ChildParachainAsNative<ParachainOrigin, Origin>(
	PhantomData<(ParachainOrigin, Origin)>
);
impl<
	ParachainOrigin: From<u32>,
	Origin: From<ParachainOrigin>,
> ConvertOrigin<Origin> for ChildParachainAsNative<ParachainOrigin, Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Native, MultiLocation::X1(Junction::Parachain { id }))
			=> Ok(Origin::from(ParachainOrigin::from(id))),
			(_, origin) => Err(origin),
		}
	}
}

pub struct SiblingParachainAsNative<ParachainOrigin, Origin>(
	PhantomData<(ParachainOrigin, Origin)>
);
impl<
	ParachainOrigin: From<u32>,
	Origin: From<ParachainOrigin>,
> ConvertOrigin<Origin> for SiblingParachainAsNative<ParachainOrigin, Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Native, MultiLocation::X2(Junction::Parent, Junction::Parachain { id }))
			=> Ok(Origin::from(ParachainOrigin::from(id))),
			(_, origin) => Err(origin),
		}
	}
}

// Our Relay-chain has a native origin given by the `Get`ter.
pub struct RelayChainAsNative<RelayOrigin, Origin>(
	PhantomData<(RelayOrigin, Origin)>
);
impl<
	RelayOrigin: Get<Origin>,
	Origin,
> ConvertOrigin<Origin> for RelayChainAsNative<RelayOrigin, Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Native, MultiLocation::X1(Junction::Parent)) => Ok(RelayOrigin::get()),
			(_, origin) => Err(origin),
		}
	}
}

pub struct SignedAccountId32AsNative<Network, Origin>(
	PhantomData<(Network, Origin)>
);
impl<
	Network: Get<NetworkId>,
	Origin: OriginTrait,
> ConvertOrigin<Origin> for SignedAccountId32AsNative<Network, Origin> where
	Origin::AccountId: From<[u8; 32]>,
{
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Native, MultiLocation::X1(Junction::AccountId32 { id, network }))
			if matches!(network, NetworkId::Any) || network == Network::get()
			=> Ok(Origin::signed(id.into())),
			(_, origin) => Err(origin),
		}
	}
}

pub struct SignedAccountKey20AsNative<Network, Origin>(
	PhantomData<(Network, Origin)>
);
impl<
	Network: Get<NetworkId>,
	Origin: OriginTrait
> ConvertOrigin<Origin> for SignedAccountKey20AsNative<Network, Origin> where
	Origin::AccountId: From<[u8; 20]>,
{
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Native, MultiLocation::X1(Junction::AccountKey20 { key, network }))
				if matches!(network, NetworkId::Any) || network == Network::get() =>
			{
				Ok(Origin::signed(key.into()))
			}
			(_, origin) => Err(origin),
		}
	}
}
