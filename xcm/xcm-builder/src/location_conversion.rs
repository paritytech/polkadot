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
use parity_scale_codec::{Decode, Encode};
use sp_io::hashing::blake2_256;
use sp_runtime::traits::{AccountIdConversion, TrailingZeroInput};
use sp_std::{borrow::Borrow, marker::PhantomData};
use xcm::latest::prelude::*;
use xcm_executor::traits::Convert;

/// Prefix for generating alias account for accounts coming  
/// from chains that use 32 byte long representations.
pub const FOREIGN_CHAIN_PREFIX_PARA_32: [u8; 37] = *b"ForeignChainAliasAccountPrefix_Para32";

/// Prefix for generating alias account for accounts coming  
/// from chains that use 20 byte long representations.
pub const FOREIGN_CHAIN_PREFIX_PARA_20: [u8; 37] = *b"ForeignChainAliasAccountPrefix_Para20";

/// Prefix for generating alias account for accounts coming  
/// from the relay chain using 32 byte long representations.
pub const FOREIGN_CHAIN_PREFIX_RELAY: [u8; 36] = *b"ForeignChainAliasAccountPrefix_Relay";

/// This converter will for a given `AccountId32`/`AccountKey20`
/// always generate the same "remote" account for a specific
/// sending chain.
/// I.e. the user gets the same remote account
/// on every consuming para-chain and relay chain.
///
/// Can be used as a converter in `SovereignSignedViaLocation`
pub struct ForeignChainAliasAccount<AccountId>(PhantomData<AccountId>);
impl<AccountId: From<[u8; 32]> + Clone> Convert<MultiLocation, AccountId>
	for ForeignChainAliasAccount<AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		let entropy = match location.borrow() {
			// Used on the relay chain for sending paras that use 32 byte accounts
			MultiLocation {
				parents: 0,
				interior: X2(Parachain(para_id), AccountId32 { id, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_32(para_id, id),

			// Used on the relay chain for sending paras that use 20 byte accounts
			MultiLocation {
				parents: 0,
				interior: X2(Parachain(para_id), AccountKey20 { key, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_20(para_id, key),

			// Used on para-chain for sending paras that use 32 byte accounts
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(para_id), AccountId32 { id, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_32(para_id, id),

			// Used on para-chain for sending paras that use 20 byte accounts
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(para_id), AccountKey20 { key, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_20(para_id, key),

			// Used on para-chain for sending from the relay chain
			MultiLocation { parents: 1, interior: X1(AccountId32 { id, .. }) } =>
				ForeignChainAliasAccount::<AccountId>::from_relay_32(id),

			// No other conversions provided
			_ => return Err(()),
		};

		Ok(entropy.into())
	}

	fn reverse_ref(_: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		Err(())
	}
}

impl<AccountId> ForeignChainAliasAccount<AccountId> {
	fn from_para_32(para_id: &u32, id: &[u8; 32]) -> [u8; 32] {
		(FOREIGN_CHAIN_PREFIX_PARA_32, para_id, id).using_encoded(blake2_256)
	}

	fn from_para_20(para_id: &u32, id: &[u8; 20]) -> [u8; 32] {
		(FOREIGN_CHAIN_PREFIX_PARA_20, para_id, id).using_encoded(blake2_256)
	}

	fn from_relay_32(id: &[u8; 32]) -> [u8; 32] {
		(FOREIGN_CHAIN_PREFIX_RELAY, id).using_encoded(blake2_256)
	}
}

pub struct Account32Hash<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<Option<NetworkId>>, AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone>
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
/// parent `AccountId`.
pub struct ParentIsPreset<AccountId>(PhantomData<AccountId>);
impl<AccountId: Decode + Eq + Clone> Convert<MultiLocation, AccountId>
	for ParentIsPreset<AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		if location.borrow().contains_parents_only(1) {
			Ok(b"Parent"
				.using_encoded(|b| AccountId::decode(&mut TrailingZeroInput::new(b)))
				.expect("infinite length input; no invalid inputs for type; qed"))
		} else {
			Err(())
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		let parent_account = b"Parent"
			.using_encoded(|b| AccountId::decode(&mut TrailingZeroInput::new(b)))
			.expect("infinite length input; no invalid inputs for type; qed");
		if who.borrow() == &parent_account {
			Ok(Parent.into())
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
		match location.borrow() {
			MultiLocation { parents: 0, interior: X1(Parachain(id)) } =>
				Ok(ParaId::from(*id).into_account_truncating()),
			_ => Err(()),
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if let Some(id) = ParaId::try_from_account(who.borrow()) {
			Ok(Parachain(id.into()).into())
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
		match location.borrow() {
			MultiLocation { parents: 1, interior: X1(Parachain(id)) } =>
				Ok(ParaId::from(*id).into_account_truncating()),
			_ => Err(()),
		}
	}

	fn reverse_ref(who: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		if let Some(id) = ParaId::try_from_account(who.borrow()) {
			Ok(MultiLocation::new(1, X1(Parachain(id.into()))))
		} else {
			Err(())
		}
	}
}

/// Extracts the `AccountId32` from the passed `location` if the network matches.
pub struct AccountId32Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<Option<NetworkId>>, AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone>
	Convert<MultiLocation, AccountId> for AccountId32Aliases<Network, AccountId>
{
	fn convert(location: MultiLocation) -> Result<AccountId, MultiLocation> {
		let id = match location {
			MultiLocation { parents: 0, interior: X1(AccountId32 { id, network: None }) } => id,
			MultiLocation { parents: 0, interior: X1(AccountId32 { id, network }) }
				if network == Network::get() =>
				id,
			_ => return Err(location),
		};
		Ok(id.into())
	}

	fn reverse(who: AccountId) -> Result<MultiLocation, AccountId> {
		Ok(AccountId32 { id: who.into(), network: Network::get() }.into())
	}
}

pub struct AccountKey20Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<Option<NetworkId>>, AccountId: From<[u8; 20]> + Into<[u8; 20]> + Clone>
	Convert<MultiLocation, AccountId> for AccountKey20Aliases<Network, AccountId>
{
	fn convert(location: MultiLocation) -> Result<AccountId, MultiLocation> {
		let key = match location {
			MultiLocation { parents: 0, interior: X1(AccountKey20 { key, network: None }) } => key,
			MultiLocation { parents: 0, interior: X1(AccountKey20 { key, network }) }
				if network == Network::get() =>
				key,
			_ => return Err(location),
		};
		Ok(key.into())
	}

	fn reverse(who: AccountId) -> Result<MultiLocation, AccountId> {
		let j = AccountKey20 { key: who.into(), network: Network::get() };
		Ok(j.into())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use frame_support::parameter_types;
	use xcm::latest::Junction;

	fn account20() -> Junction {
		AccountKey20 { network: None, key: Default::default() }
	}

	fn account32() -> Junction {
		AccountId32 { network: None, id: Default::default() }
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
	// context (root to source): para_1/account20_default/account20_default
	// =>
	// output (target to source): ../../para_1/account20_default/account20_default
	#[test]
	fn inverter_works_in_tree() {
		parameter_types! {
			pub UniversalLocation: InteriorMultiLocation = X3(Parachain(1), account20(), account20());
		}

		let input = MultiLocation::new(3, X2(Parachain(2), account32()));
		let inverted = UniversalLocation::get().invert_target(&input).unwrap();
		assert_eq!(inverted, MultiLocation::new(2, X3(Parachain(1), account20(), account20())));
	}

	// Network Topology
	//                                     v Source
	// Relay -> Para 1 -> SmartContract -> Account
	//          ^ Target
	#[test]
	fn inverter_uses_context_as_inverted_location() {
		parameter_types! {
			pub UniversalLocation: InteriorMultiLocation = X2(account20(), account20());
		}

		let input = MultiLocation::grandparent();
		let inverted = UniversalLocation::get().invert_target(&input).unwrap();
		assert_eq!(inverted, X2(account20(), account20()).into());
	}

	// Network Topology
	//                                        v Source
	// Relay -> Para 1 -> CollectivePallet -> Plurality
	//          ^ Target
	#[test]
	fn inverter_uses_only_child_on_missing_context() {
		parameter_types! {
			pub UniversalLocation: InteriorMultiLocation = PalletInstance(5).into();
		}

		let input = MultiLocation::grandparent();
		let inverted = UniversalLocation::get().invert_target(&input).unwrap();
		assert_eq!(inverted, (OnlyChild, PalletInstance(5)).into());
	}

	#[test]
	fn inverter_errors_when_location_is_too_large() {
		parameter_types! {
			pub UniversalLocation: InteriorMultiLocation = Here;
		}

		let input = MultiLocation { parents: 99, interior: X1(Parachain(88)) };
		let inverted = UniversalLocation::get().invert_target(&input);
		assert_eq!(inverted, Err(()));
	}

	#[test]
	fn remote_account_convert_on_para_sending_para_32() {
		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(1), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				43, 205, 192, 244, 57, 93, 151, 208, 156, 78, 24, 118, 192, 126, 212, 77, 158, 137,
				98, 101, 12, 20, 129, 113, 190, 51, 32, 217, 39, 113, 109, 6
			],
			rem_1
		);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(
				Parachain(1),
				AccountId32 { network: Some(NetworkId::Polkadot), id: [0u8; 32] },
			),
		};

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(2), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				2, 64, 114, 208, 208, 93, 134, 10, 11, 103, 214, 240, 226, 215, 103, 4, 54, 15,
				225, 104, 116, 206, 195, 130, 148, 31, 166, 234, 208, 133, 205, 193
			],
			rem_2
		);

		assert_ne!(rem_1, rem_2);
	}

	#[test]
	fn remote_account_convert_on_para_sending_para_20() {
		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(1), AccountKey20 { network: None, key: [0u8; 20] }),
		};
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				220, 227, 86, 34, 106, 215, 195, 254, 66, 227, 147, 109, 29, 94, 166, 218, 167, 12,
				173, 249, 210, 12, 160, 175, 74, 47, 66, 101, 57, 0, 6, 237
			],
			rem_1
		);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(
				Parachain(1),
				AccountKey20 { network: Some(NetworkId::Polkadot), key: [0u8; 20] },
			),
		};

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(2), AccountKey20 { network: None, key: [0u8; 20] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				36, 151, 177, 242, 57, 63, 152, 88, 86, 120, 68, 182, 184, 241, 226, 146, 160, 58,
				117, 88, 68, 252, 215, 207, 123, 47, 230, 70, 226, 128, 93, 95
			],
			rem_2
		);

		assert_ne!(rem_1, rem_2);
	}

	#[test]
	fn remote_account_convert_on_para_sending_relay() {
		let mul = MultiLocation {
			parents: 1,
			interior: X1(AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				94, 196, 87, 187, 9, 80, 218, 5, 197, 191, 254, 209, 97, 71, 157, 179, 89, 177,
				143, 116, 89, 101, 166, 229, 63, 192, 113, 84, 33, 75, 53, 180
			],
			rem_1
		);

		let mul = MultiLocation {
			parents: 1,
			interior: X1(AccountId32 { network: Some(NetworkId::Polkadot), id: [0u8; 32] }),
		};

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 1,
			interior: X1(AccountId32 { network: None, id: [1u8; 32] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				80, 50, 225, 2, 93, 166, 226, 77, 90, 153, 212, 206, 248, 60, 123, 128, 229, 23,
				251, 53, 66, 248, 216, 139, 5, 54, 72, 98, 165, 23, 45, 56
			],
			rem_2
		);

		assert_ne!(rem_1, rem_2);
	}

	#[test]
	fn remote_account_convert_on_relay_sending_para_20() {
		let mul = MultiLocation {
			parents: 0,
			interior: X2(Parachain(1), AccountKey20 { network: None, key: [0u8; 20] }),
		};
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				220, 227, 86, 34, 106, 215, 195, 254, 66, 227, 147, 109, 29, 94, 166, 218, 167, 12,
				173, 249, 210, 12, 160, 175, 74, 47, 66, 101, 57, 0, 6, 237
			],
			rem_1
		);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(
				Parachain(1),
				AccountKey20 { network: Some(NetworkId::Polkadot), key: [0u8; 20] },
			),
		};

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 0,
			interior: X2(Parachain(2), AccountKey20 { network: None, key: [0u8; 20] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				36, 151, 177, 242, 57, 63, 152, 88, 86, 120, 68, 182, 184, 241, 226, 146, 160, 58,
				117, 88, 68, 252, 215, 207, 123, 47, 230, 70, 226, 128, 93, 95
			],
			rem_2
		);

		assert_ne!(rem_1, rem_2);
	}

	#[test]
	fn remote_account_convert_on_relay_sending_para_32() {
		let mul = MultiLocation {
			parents: 0,
			interior: X2(Parachain(1), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				43, 205, 192, 244, 57, 93, 151, 208, 156, 78, 24, 118, 192, 126, 212, 77, 158, 137,
				98, 101, 12, 20, 129, 113, 190, 51, 32, 217, 39, 113, 109, 6
			],
			rem_1
		);

		let mul = MultiLocation {
			parents: 0,
			interior: X2(
				Parachain(1),
				AccountId32 { network: Some(NetworkId::Polkadot), id: [0u8; 32] },
			),
		};

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 0,
			interior: X2(Parachain(2), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				2, 64, 114, 208, 208, 93, 134, 10, 11, 103, 214, 240, 226, 215, 103, 4, 54, 15,
				225, 104, 116, 206, 195, 130, 148, 31, 166, 234, 208, 133, 205, 193
			],
			rem_2
		);

		assert_ne!(rem_1, rem_2);
	}

	#[test]
	fn remote_account_fails_with_bad_multilocation() {
		let mul = MultiLocation {
			parents: 1,
			interior: X1(AccountKey20 { network: None, key: [0u8; 20] }),
		};
		assert!(ForeignChainAliasAccount::<[u8; 32]>::convert(mul).is_err());
	}
}
