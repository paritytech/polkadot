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

use frame_support::traits::Get;
use parity_scale_codec::Encode;
use sp_io::hashing::blake2_256;
use sp_runtime::traits::AccountIdConversion;
use sp_std::{borrow::Borrow, marker::PhantomData};
use xcm::latest::{Junction, Junctions, MultiLocation, NetworkId};
use xcm_executor::traits::{Convert, InvertLocation};

pub struct Account32Hash<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<NetworkId>, AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone>
	Convert<MultiLocation, AccountId> for Account32Hash<Network, AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		Ok(("multiloc", location.borrow()).using_encoded(blake2_256).into())
	}

	fn reverse_ref(_: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		Err(())
	}
}

/// A [`MultiLocation`] consisting of a single `Parent` [`Junction`] will be converted to the
/// default value of `AccountId` (e.g. all zeros for `AccountId32`).
pub struct ParentIsDefault<AccountId>(PhantomData<AccountId>);
impl<AccountId: Default + Eq + Clone> Convert<MultiLocation, AccountId>
	for ParentIsDefault<AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		if location.borrow().parent_count() == 1 && location.borrow().interior().len() == 0 {
			Ok(AccountId::default())
		} else {
			Err(())
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if who.borrow() == &AccountId::default() {
			Ok(MultiLocation::with_parents::<1>())
		} else {
			Err(())
		}
	}
}

pub struct ChildParachainConvertsVia<ParaId, AccountId>(PhantomData<(ParaId, AccountId)>);
impl<ParaId: From<u32> + Into<u32> + AccountIdConversion<AccountId>, AccountId: Clone>
	Convert<MultiLocation, AccountId> for ChildParachainConvertsVia<ParaId, AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		let location = location.borrow();
		match location.interior() {
			Junctions::X1(Junction::Parachain(id)) if location.parent_count() == 0 =>
				Ok(ParaId::from(*id).into_account()),
			_ => Err(()),
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
impl<ParaId: From<u32> + Into<u32> + AccountIdConversion<AccountId>, AccountId: Clone>
	Convert<MultiLocation, AccountId> for SiblingParachainConvertsVia<ParaId, AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		let location = location.borrow();
		match location.interior() {
			Junctions::X1(Junction::Parachain(id)) if location.parent_count() == 1 =>
				Ok(ParaId::from(*id).into_account()),
			_ => Err(()),
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if let Some(id) = ParaId::try_from_account(who.borrow()) {
			Ok(MultiLocation::new(1, Junctions::X1(Junction::Parachain(id.into())))
				.expect("well-formed MultiLocation; qed"))
		} else {
			Err(())
		}
	}
}

/// Extracts the `AccountId32` from the passed `location` if the network matches.
pub struct AccountId32Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<NetworkId>, AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone>
	Convert<MultiLocation, AccountId> for AccountId32Aliases<Network, AccountId>
{
	fn convert(location: MultiLocation) -> Result<AccountId, MultiLocation> {
		let id = match location.interior() {
			Junctions::X1(Junction::AccountId32 { id, network: NetworkId::Any })
				if location.parent_count() == 0 =>
				*id,
			Junctions::X1(Junction::AccountId32 { id, network })
				if network == &Network::get() && location.parent_count() == 0 =>
				*id,
			_ => return Err(location),
		};
		Ok(id.into())
	}

	fn reverse(who: AccountId) -> Result<MultiLocation, AccountId> {
		Ok(Junction::AccountId32 { id: who.into(), network: Network::get() }.into())
	}
}

pub struct AccountKey20Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<NetworkId>, AccountId: From<[u8; 20]> + Into<[u8; 20]> + Clone>
	Convert<MultiLocation, AccountId> for AccountKey20Aliases<Network, AccountId>
{
	fn convert(location: MultiLocation) -> Result<AccountId, MultiLocation> {
		let key = match location.interior() {
			Junctions::X1(Junction::AccountKey20 { key, network: NetworkId::Any })
				if location.parent_count() == 0 =>
				*key,
			Junctions::X1(Junction::AccountKey20 { key, network })
				if network == &Network::get() && location.parent_count() == 0 =>
				*key,
			_ => return Err(location),
		};
		Ok(key.into())
	}

	fn reverse(who: AccountId) -> Result<MultiLocation, AccountId> {
		let j = Junction::AccountKey20 { key: who.into(), network: Network::get() };
		Ok(j.into())
	}
}

/// Simple location inverter; give it this location's ancestry and it'll figure out the inverted
/// location.
///
/// # Example
/// ## Network Topology
/// ```txt
///                    v Source
/// Relay -> Para 1 -> Account20
///       -> Para 2 -> Account32
///                    ^ Target
/// ```
/// ```rust
/// # use frame_support::parameter_types;
/// # use xcm::latest::{MultiLocation, Junction::*, Junctions::{self, *}, NetworkId::Any};
/// # use xcm_builder::LocationInverter;
/// # use xcm_executor::traits::InvertLocation;
/// # fn main() {
/// parameter_types!{
///     pub Ancestry: MultiLocation = X2(
///         Parachain(1),
///         AccountKey20 { network: Any, key: Default::default() },
///     ).into();
/// }
///
/// let input = MultiLocation::new(2, X2(Parachain(2), AccountId32 { network: Any, id: Default::default() })).unwrap();
/// let inverted = LocationInverter::<Ancestry>::invert_location(&input);
/// assert_eq!(inverted, MultiLocation::new(
///     2,
///     X2(Parachain(1), AccountKey20 { network: Any, key: Default::default() }),
/// ).unwrap());
/// # }
/// ```
pub struct LocationInverter<Ancestry>(PhantomData<Ancestry>);
impl<Ancestry: Get<MultiLocation>> InvertLocation for LocationInverter<Ancestry> {
	fn invert_location(location: &MultiLocation) -> MultiLocation {
		let mut ancestry = Ancestry::get();
		let mut junctions = Junctions::Null;
		for _ in 0..location.parent_count() {
			junctions = junctions
				.pushed_with(ancestry.take_first_interior().unwrap_or(Junction::OnlyChild))
				.expect("ancestry is well-formed and has less than 8 non-parent junctions; qed");
		}
		let parents = location.interior().len() as u8;
		MultiLocation::new(parents, junctions)
			.expect("parents + junctions len must equal location len; qed")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use frame_support::parameter_types;
	use xcm::latest::{Junction::*, Junctions::*, MultiLocation, NetworkId::Any};

	fn account20() -> Junction {
		AccountKey20 { network: Any, key: Default::default() }
	}

	fn account32() -> Junction {
		AccountId32 { network: Any, id: Default::default() }
	}

	// Network Topology
	//                                     v Source
	// Relay -> Para 1 -> SmartContract -> Account
	//       -> Para 2 -> Account
	//                    ^ Target
	//
	// Inputs and outputs written as file paths:
	//
	// input location (source to target): ../../../para_2/account32_default
	// ancestry (root to source): para_1/account20_default/account20_default
	// =>
	// output (target to source): ../../para_1/account20_default/account20_default
	#[test]
	fn inverter_works_in_tree() {
		parameter_types! {
			pub Ancestry: MultiLocation = X3(Parachain(1), account20(), account20()).into();
		}

		let input = MultiLocation::new(3, X2(Parachain(2), account32())).unwrap();
		let inverted = LocationInverter::<Ancestry>::invert_location(&input);
		assert_eq!(
			inverted,
			MultiLocation::new(2, X3(Parachain(1), account20(), account20())).unwrap()
		);
	}

	// Network Topology
	//                                     v Source
	// Relay -> Para 1 -> SmartContract -> Account
	//          ^ Target
	#[test]
	fn inverter_uses_ancestry_as_inverted_location() {
		parameter_types! {
			pub Ancestry: MultiLocation = X2(account20(), account20()).into();
		}

		let input = MultiLocation::new(2, Null).unwrap();
		let inverted = LocationInverter::<Ancestry>::invert_location(&input);
		assert_eq!(inverted, X2(account20(), account20()).into());
	}

	// Network Topology
	//                                        v Source
	// Relay -> Para 1 -> CollectivePallet -> Plurality
	//          ^ Target
	#[test]
	fn inverter_uses_only_child_on_missing_ancestry() {
		parameter_types! {
			pub Ancestry: MultiLocation = X1(PalletInstance(5)).into();
		}

		let input = MultiLocation::new(2, Null).unwrap();
		let inverted = LocationInverter::<Ancestry>::invert_location(&input);
		assert_eq!(inverted, X2(PalletInstance(5), OnlyChild).into());
	}
}
