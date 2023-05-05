// Copyright (C) Parity Technologies (UK) Ltd.
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
///
/// ## Example
/// Assuming the following network layout.
///
/// ```notrust
///              R
///           /    \
///          /      \   
///        P1       P2
///        / \       / \
///       /   \     /   \
///     P1.1 P1.2  P2.1  P2.2
/// ```
/// Then a given account A will have the same alias accounts in the
/// same plane. So, it is important which chain account A acts from.
/// E.g.
/// * From P1.2 A will act as
///    * hash(ParaPrefix, A, 1, 1) on P1.2
///    * hash(ParaPrefix, A, 1, 0) on P1
/// * From P1 A will act as
///    * hash(RelayPrefix, A, 1) on P1.2 & P1.1
///    * hash(ParaPrefix, A, 1, 1) on P2
///    * hash(ParaPrefix, A, 1, 0) on R
///
/// Note that the alias accounts have overlaps but never on the same
/// chain when the sender comes from different chains.
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
			} => ForeignChainAliasAccount::<AccountId>::from_para_32(para_id, id, 0),

			// Used on the relay chain for sending paras that use 20 byte accounts
			MultiLocation {
				parents: 0,
				interior: X2(Parachain(para_id), AccountKey20 { key, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_20(para_id, key, 0),

			// Used on para-chain for sending paras that use 32 byte accounts
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(para_id), AccountId32 { id, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_32(para_id, id, 1),

			// Used on para-chain for sending paras that use 20 byte accounts
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(para_id), AccountKey20 { key, .. }),
			} => ForeignChainAliasAccount::<AccountId>::from_para_20(para_id, key, 1),

			// Used on para-chain for sending from the relay chain
			MultiLocation { parents: 1, interior: X1(AccountId32 { id, .. }) } =>
				ForeignChainAliasAccount::<AccountId>::from_relay_32(id, 1),

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
	fn from_para_32(para_id: &u32, id: &[u8; 32], parents: u8) -> [u8; 32] {
		(FOREIGN_CHAIN_PREFIX_PARA_32, para_id, id, parents).using_encoded(blake2_256)
	}

	fn from_para_20(para_id: &u32, id: &[u8; 20], parents: u8) -> [u8; 32] {
		(FOREIGN_CHAIN_PREFIX_PARA_20, para_id, id, parents).using_encoded(blake2_256)
	}

	fn from_relay_32(id: &[u8; 32], parents: u8) -> [u8; 32] {
		(FOREIGN_CHAIN_PREFIX_RELAY, id, parents).using_encoded(blake2_256)
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
				181, 186, 132, 152, 52, 210, 226, 199, 8, 235, 213, 242, 94, 70, 250, 170, 19, 163,
				196, 102, 245, 14, 172, 184, 2, 148, 108, 87, 230, 163, 204, 32
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
				183, 188, 66, 169, 82, 250, 45, 30, 142, 119, 184, 55, 177, 64, 53, 114, 12, 147,
				128, 10, 60, 45, 41, 193, 87, 18, 86, 49, 127, 233, 243, 143
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
				210, 60, 37, 255, 116, 38, 221, 26, 85, 82, 252, 125, 220, 19, 41, 91, 185, 69,
				102, 83, 120, 63, 15, 212, 74, 141, 82, 203, 187, 212, 77, 120
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
				197, 16, 31, 199, 234, 80, 166, 55, 178, 135, 95, 48, 19, 128, 9, 167, 51, 99, 215,
				147, 94, 171, 28, 157, 29, 107, 240, 22, 10, 104, 99, 186
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
				227, 12, 152, 241, 220, 53, 26, 27, 1, 167, 167, 214, 61, 161, 255, 96, 56, 16,
				221, 59, 47, 45, 40, 193, 88, 92, 4, 167, 164, 27, 112, 99
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
				143, 195, 87, 73, 129, 2, 163, 211, 239, 51, 55, 235, 82, 173, 162, 206, 158, 237,
				166, 73, 254, 62, 131, 6, 170, 241, 209, 116, 105, 69, 29, 226
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
				25, 251, 15, 92, 148, 141, 236, 238, 50, 108, 133, 56, 118, 11, 250, 122, 81, 160,
				104, 160, 97, 200, 210, 49, 208, 142, 64, 144, 24, 110, 246, 101
			],
			rem_1
		);

		let mul = MultiLocation {
			parents: 0,
			interior: X2(Parachain(2), AccountKey20 { network: None, key: [0u8; 20] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert(mul).unwrap();

		assert_eq!(
			[
				88, 157, 224, 235, 76, 88, 201, 143, 206, 227, 14, 192, 177, 245, 75, 62, 41, 10,
				107, 182, 61, 57, 239, 112, 43, 151, 58, 111, 150, 153, 234, 189
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
				45, 120, 232, 0, 226, 49, 106, 48, 65, 181, 184, 147, 224, 235, 198, 152, 183, 156,
				67, 57, 67, 67, 187, 104, 171, 23, 140, 21, 183, 152, 63, 20
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
				97, 119, 110, 66, 239, 113, 96, 234, 127, 92, 66, 204, 53, 129, 33, 119, 213, 192,
				171, 100, 139, 51, 39, 62, 196, 163, 16, 213, 160, 44, 100, 228
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
