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

use sp_std::{marker::PhantomData, borrow::Borrow};
use sp_io::hashing::blake2_256;
use sp_runtime::traits::AccountIdConversion;
use frame_support::traits::Get;
use parity_scale_codec::Encode;
use xcm::v0::{MultiLocation, NetworkId, Junction};
use xcm_executor::traits::{InvertLocation, Convert};

pub struct Account32Hash<Network, AccountId>(PhantomData<(Network, AccountId)>);

impl<
	Network: Get<NetworkId>,
	AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone,
> Convert<MultiLocation, AccountId> for Account32Hash<Network, AccountId> {
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		Ok(("multiloc", location.borrow()).using_encoded(blake2_256).into())
	}

	fn reverse_ref(_: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		Err(())
	}
}

pub struct ParentIsDefault<AccountId>(PhantomData<AccountId>);
impl<
	AccountId: Default + Eq + Clone,
> Convert<MultiLocation, AccountId> for ParentIsDefault<AccountId> {
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		if let &MultiLocation::X1(Junction::Parent) = location.borrow() {
			Ok(AccountId::default())
		} else {
			Err(())
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if who.borrow() == &AccountId::default() {
			Ok(Junction::Parent.into())
		} else {
			Err(())
		}
	}
}

pub struct ChildParachainConvertsVia<ParaId, AccountId>(PhantomData<(ParaId, AccountId)>);
impl<
	ParaId: From<u32> + Into<u32> + AccountIdConversion<AccountId>,
	AccountId: Clone,
> Convert<MultiLocation, AccountId> for ChildParachainConvertsVia<ParaId, AccountId> {
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		if let &MultiLocation::X1(Junction::Parachain(id)) = location.borrow() {
			Ok(ParaId::from(id).into_account())
		} else {
			Err(())
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if let Some(id) = ParaId::try_from_account(who.borrow()) {
			Ok(Junction::Parachain(id.into()).into())
		} else {
			Err(())
		}
	}
}

pub struct SiblingParachainConvertsVia<ParaId, AccountId>(PhantomData<(ParaId, AccountId)>);

impl<
	ParaId: From<u32> + Into<u32> + AccountIdConversion<AccountId>,
	AccountId: Clone,
> Convert<MultiLocation, AccountId> for SiblingParachainConvertsVia<ParaId, AccountId> {
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		if let &MultiLocation::X2(Junction::Parent, Junction::Parachain(id)) = location.borrow() {
			Ok(ParaId::from(id).into_account())
		} else {
			Err(())
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if let Some(id) = ParaId::try_from_account(who.borrow()) {
			Ok([Junction::Parent, Junction::Parachain(id.into())].into())
		} else {
			Err(())
		}
	}
}

pub struct AccountId32Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<
	Network: Get<NetworkId>,
	AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone,
> Convert<MultiLocation, AccountId> for AccountId32Aliases<Network, AccountId> {
	fn convert(location: MultiLocation) -> Result<AccountId, MultiLocation> {
		let id = match location {
			MultiLocation::X1(Junction::AccountId32 { id, network: NetworkId::Any }) => id,
			MultiLocation::X1(Junction::AccountId32 { id, network }) if &network == &Network::get() => id,
			l => return Err(l),
		};
		Ok(id.into())
	}

	fn reverse(who: AccountId) -> Result<MultiLocation, AccountId> {
		Ok(Junction::AccountId32 { id: who.into(), network: Network::get() }.into())
	}
}

pub struct AccountKey20Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<
	Network: Get<NetworkId>,
	AccountId: From<[u8; 20]> + Into<[u8; 20]> + Clone,
> Convert<MultiLocation, AccountId> for AccountKey20Aliases<Network, AccountId> {
	fn convert(location: MultiLocation) -> Result<AccountId, MultiLocation> {
		let key = match location {
			MultiLocation::X1(Junction::AccountKey20 { key, network: NetworkId::Any }) => key,
			MultiLocation::X1(Junction::AccountKey20 { key, network }) if &network == &Network::get() => key,
			l => return Err(l),
		};
		Ok(key.into())
	}

	fn reverse(who: AccountId) -> Result<MultiLocation, AccountId> {
		let j = Junction::AccountKey20 { key: who.into(), network: Network::get() };
		Ok(j.into())
	}
}

/// Simple location inverter; give it this location's ancestry and it'll figure out the inverted location.
pub struct LocationInverter<Ancestry>(PhantomData<Ancestry>);
impl<Ancestry: Get<MultiLocation>> InvertLocation for LocationInverter<Ancestry> {
	fn invert_location(location: &MultiLocation) -> MultiLocation {
		let mut ancestry = Ancestry::get();
		let mut result = location.clone();
		for (i, j) in location.iter_rev()
			.map(|j| match j {
				Junction::Parent => ancestry.take_first().unwrap_or(Junction::OnlyChild),
				_ => Junction::Parent,
			})
			.enumerate()
		{
			*result.at_mut(i).expect("location and result begin equal; same size; qed") = j;
		}
		result
	}
}
