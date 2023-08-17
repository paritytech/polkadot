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

use crate::universal_exports::ensure_is_remote;
use frame_support::traits::Get;
use parity_scale_codec::{Compact, Decode, Encode};
use sp_io::hashing::blake2_256;
use sp_runtime::traits::{AccountIdConversion, Convert, TrailingZeroInput};
use sp_std::{marker::PhantomData, prelude::*};
use xcm::latest::prelude::*;
use xcm_executor::traits::ConvertLocation;

/// Means of converting a location into a stable and unique descriptive identifier.
pub trait DescribeLocation {
	/// Create a description of the given `location` if possible. No two locations should have the
	/// same descriptor.
	fn describe_location(location: &MultiLocation) -> Option<Vec<u8>>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl DescribeLocation for Tuple {
	fn describe_location(l: &MultiLocation) -> Option<Vec<u8>> {
		for_tuples!( #(
			match Tuple::describe_location(l) {
				Some(result) => return Some(result),
				None => {},
			}
		)* );
		None
	}
}

pub struct DescribeTerminus;
impl DescribeLocation for DescribeTerminus {
	fn describe_location(l: &MultiLocation) -> Option<Vec<u8>> {
		match (l.parents, &l.interior) {
			(0, Here) => Some(Vec::new()),
			_ => return None,
		}
	}
}

pub struct DescribePalletTerminal;
impl DescribeLocation for DescribePalletTerminal {
	fn describe_location(l: &MultiLocation) -> Option<Vec<u8>> {
		match (l.parents, &l.interior) {
			(0, X1(PalletInstance(i))) =>
				Some((b"Pallet", Compact::<u32>::from(*i as u32)).encode()),
			_ => return None,
		}
	}
}

pub struct DescribeAccountId32Terminal;
impl DescribeLocation for DescribeAccountId32Terminal {
	fn describe_location(l: &MultiLocation) -> Option<Vec<u8>> {
		match (l.parents, &l.interior) {
			(0, X1(AccountId32 { id, .. })) => Some((b"AccountId32", id).encode()),
			_ => return None,
		}
	}
}

pub struct DescribeAccountKey20Terminal;
impl DescribeLocation for DescribeAccountKey20Terminal {
	fn describe_location(l: &MultiLocation) -> Option<Vec<u8>> {
		match (l.parents, &l.interior) {
			(0, X1(AccountKey20 { key, .. })) => Some((b"AccountKey20", key).encode()),
			_ => return None,
		}
	}
}

pub type DescribeAccountIdTerminal = (DescribeAccountId32Terminal, DescribeAccountKey20Terminal);

pub type DescribeAllTerminal = (
	DescribeTerminus,
	DescribePalletTerminal,
	DescribeAccountId32Terminal,
	DescribeAccountKey20Terminal,
);

pub struct DescribeFamily<DescribeInterior>(PhantomData<DescribeInterior>);
impl<Suffix: DescribeLocation> DescribeLocation for DescribeFamily<Suffix> {
	fn describe_location(l: &MultiLocation) -> Option<Vec<u8>> {
		match (l.parents, l.interior.first()) {
			(0, Some(Parachain(index))) => {
				let tail = l.interior.split_first().0;
				let interior = Suffix::describe_location(&tail.into())?;
				Some((b"ChildChain", Compact::<u32>::from(*index), interior).encode())
			},
			(1, Some(Parachain(index))) => {
				let tail = l.interior.split_first().0;
				let interior = Suffix::describe_location(&tail.into())?;
				Some((b"SiblingChain", Compact::<u32>::from(*index), interior).encode())
			},
			(1, _) => {
				let tail = l.interior.into();
				let interior = Suffix::describe_location(&tail)?;
				Some((b"ParentChain", interior).encode())
			},
			_ => return None,
		}
	}
}

pub struct HashedDescription<AccountId, Describe>(PhantomData<(AccountId, Describe)>);
impl<AccountId: From<[u8; 32]> + Clone, Describe: DescribeLocation> ConvertLocation<AccountId>
	for HashedDescription<AccountId, Describe>
{
	fn convert_location(value: &MultiLocation) -> Option<AccountId> {
		Some(blake2_256(&Describe::describe_location(value)?).into())
	}
}

/// This is a describer for legacy support of the `ForeignChainAliasAccount` preimage. New chains
/// are recommended to use the more extensible `HashedDescription` type.
pub struct LegacyDescribeForeignChainAccount;
impl DescribeLocation for LegacyDescribeForeignChainAccount {
	fn describe_location(location: &MultiLocation) -> Option<Vec<u8>> {
		Some(match location {
			// Used on the relay chain for sending paras that use 32 byte accounts
			MultiLocation {
				parents: 0,
				interior: X2(Parachain(para_id), AccountId32 { id, .. }),
			} => LegacyDescribeForeignChainAccount::from_para_32(para_id, id, 0),

			// Used on the relay chain for sending paras that use 20 byte accounts
			MultiLocation {
				parents: 0,
				interior: X2(Parachain(para_id), AccountKey20 { key, .. }),
			} => LegacyDescribeForeignChainAccount::from_para_20(para_id, key, 0),

			// Used on para-chain for sending paras that use 32 byte accounts
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(para_id), AccountId32 { id, .. }),
			} => LegacyDescribeForeignChainAccount::from_para_32(para_id, id, 1),

			// Used on para-chain for sending paras that use 20 byte accounts
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(para_id), AccountKey20 { key, .. }),
			} => LegacyDescribeForeignChainAccount::from_para_20(para_id, key, 1),

			// Used on para-chain for sending from the relay chain
			MultiLocation { parents: 1, interior: X1(AccountId32 { id, .. }) } =>
				LegacyDescribeForeignChainAccount::from_relay_32(id, 1),

			// No other conversions provided
			_ => return None,
		})
	}
}

/// Prefix for generating alias account for accounts coming
/// from chains that use 32 byte long representations.
pub const FOREIGN_CHAIN_PREFIX_PARA_32: [u8; 37] = *b"ForeignChainAliasAccountPrefix_Para32";

/// Prefix for generating alias account for accounts coming
/// from chains that use 20 byte long representations.
pub const FOREIGN_CHAIN_PREFIX_PARA_20: [u8; 37] = *b"ForeignChainAliasAccountPrefix_Para20";

/// Prefix for generating alias account for accounts coming
/// from the relay chain using 32 byte long representations.
pub const FOREIGN_CHAIN_PREFIX_RELAY: [u8; 36] = *b"ForeignChainAliasAccountPrefix_Relay";

impl LegacyDescribeForeignChainAccount {
	fn from_para_32(para_id: &u32, id: &[u8; 32], parents: u8) -> Vec<u8> {
		(FOREIGN_CHAIN_PREFIX_PARA_32, para_id, id, parents).encode()
	}

	fn from_para_20(para_id: &u32, id: &[u8; 20], parents: u8) -> Vec<u8> {
		(FOREIGN_CHAIN_PREFIX_PARA_20, para_id, id, parents).encode()
	}

	fn from_relay_32(id: &[u8; 32], parents: u8) -> Vec<u8> {
		(FOREIGN_CHAIN_PREFIX_RELAY, id, parents).encode()
	}
}

/// This is deprecated in favor of the more modular `HashedDescription` converter. If
/// your chain has previously used this, then you can retain backwards compatibility using
/// `HashedDescription` and a tuple with `LegacyDescribeForeignChainAccount` as the first
/// element. For example:
///
/// ```nocompile
/// pub type LocationToAccount = HashedDescription<
///   // Legacy conversion - MUST BE FIRST!
///   LegacyDescribeForeignChainAccount,
///   // Other conversions
///   DescribeTerminus,
///   DescribePalletTerminal,
/// >;
/// ```
///
/// This type is equivalent to the above but without any other conversions.
///
/// ### Old documentation
///
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
///    * hash(`ParaPrefix`, A, 1, 1) on P1.2
///    * hash(`ParaPrefix`, A, 1, 0) on P1
/// * From P1 A will act as
///    * hash(`RelayPrefix`, A, 1) on P1.2 & P1.1
///    * hash(`ParaPrefix`, A, 1, 1) on P2
///    * hash(`ParaPrefix`, A, 1, 0) on R
///
/// Note that the alias accounts have overlaps but never on the same
/// chain when the sender comes from different chains.
#[deprecated = "Use `HashedDescription<AccountId, LegacyDescribeForeignChainAccount>` instead"]
pub type ForeignChainAliasAccount<AccountId> =
	HashedDescription<AccountId, LegacyDescribeForeignChainAccount>;

pub struct Account32Hash<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<Option<NetworkId>>, AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone>
	ConvertLocation<AccountId> for Account32Hash<Network, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		Some(("multiloc", location).using_encoded(blake2_256).into())
	}
}

/// A [`MultiLocation`] consisting of a single `Parent` [`Junction`] will be converted to the
/// parent `AccountId`.
pub struct ParentIsPreset<AccountId>(PhantomData<AccountId>);
impl<AccountId: Decode + Eq + Clone> ConvertLocation<AccountId> for ParentIsPreset<AccountId> {
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		if location.contains_parents_only(1) {
			Some(
				b"Parent"
					.using_encoded(|b| AccountId::decode(&mut TrailingZeroInput::new(b)))
					.expect("infinite length input; no invalid inputs for type; qed"),
			)
		} else {
			None
		}
	}
}

pub struct ChildParachainConvertsVia<ParaId, AccountId>(PhantomData<(ParaId, AccountId)>);
impl<ParaId: From<u32> + Into<u32> + AccountIdConversion<AccountId>, AccountId: Clone>
	ConvertLocation<AccountId> for ChildParachainConvertsVia<ParaId, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		match location {
			MultiLocation { parents: 0, interior: X1(Parachain(id)) } =>
				Some(ParaId::from(*id).into_account_truncating()),
			_ => None,
		}
	}
}

pub struct SiblingParachainConvertsVia<ParaId, AccountId>(PhantomData<(ParaId, AccountId)>);
impl<ParaId: From<u32> + Into<u32> + AccountIdConversion<AccountId>, AccountId: Clone>
	ConvertLocation<AccountId> for SiblingParachainConvertsVia<ParaId, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		match location {
			MultiLocation { parents: 1, interior: X1(Parachain(id)) } =>
				Some(ParaId::from(*id).into_account_truncating()),
			_ => None,
		}
	}
}

/// Extracts the `AccountId32` from the passed `location` if the network matches.
pub struct AccountId32Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<Option<NetworkId>>, AccountId: From<[u8; 32]> + Into<[u8; 32]> + Clone>
	ConvertLocation<AccountId> for AccountId32Aliases<Network, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		let id = match *location {
			MultiLocation { parents: 0, interior: X1(AccountId32 { id, network: None }) } => id,
			MultiLocation { parents: 0, interior: X1(AccountId32 { id, network }) }
				if network == Network::get() =>
				id,
			_ => return None,
		};
		Some(id.into())
	}
}

/// Conversion implementation which converts from a `[u8; 32]`-based `AccountId` into a
/// `MultiLocation` consisting solely of a `AccountId32` junction with a fixed value for its
/// network (provided by `Network`) and the `AccountId`'s `[u8; 32]` datum for the `id`.
pub struct AliasesIntoAccountId32<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<'a, Network: Get<Option<NetworkId>>, AccountId: Clone + Into<[u8; 32]> + Clone>
	Convert<&'a AccountId, MultiLocation> for AliasesIntoAccountId32<Network, AccountId>
{
	fn convert(who: &AccountId) -> MultiLocation {
		AccountId32 { network: Network::get(), id: who.clone().into() }.into()
	}
}

pub struct AccountKey20Aliases<Network, AccountId>(PhantomData<(Network, AccountId)>);
impl<Network: Get<Option<NetworkId>>, AccountId: From<[u8; 20]> + Into<[u8; 20]> + Clone>
	ConvertLocation<AccountId> for AccountKey20Aliases<Network, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		let key = match *location {
			MultiLocation { parents: 0, interior: X1(AccountKey20 { key, network: None }) } => key,
			MultiLocation { parents: 0, interior: X1(AccountKey20 { key, network }) }
				if network == Network::get() =>
				key,
			_ => return None,
		};
		Some(key.into())
	}
}

/// Converts a location which is a top-level relay chain (which provides its own consensus) into a
/// 32-byte `AccountId`.
///
/// This will always result in the *same account ID* being returned for the same Relay-chain,
/// regardless of the relative security of this Relay-chain compared to the local chain.
///
/// Note: No distinction is made between the cases when the given `UniversalLocation` lies within
/// the same consensus system (i.e. is itself or a parent) and when it is a foreign consensus
/// system.
pub struct GlobalConsensusConvertsFor<UniversalLocation, AccountId>(
	PhantomData<(UniversalLocation, AccountId)>,
);
impl<UniversalLocation: Get<InteriorMultiLocation>, AccountId: From<[u8; 32]> + Clone>
	ConvertLocation<AccountId> for GlobalConsensusConvertsFor<UniversalLocation, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		let universal_source = UniversalLocation::get();
		log::trace!(
			target: "xcm::location_conversion",
			"GlobalConsensusConvertsFor universal_source: {:?}, location: {:?}",
			universal_source, location,
		);
		let (remote_network, remote_location) =
			ensure_is_remote(universal_source, *location).ok()?;

		match remote_location {
			Here => Some(AccountId::from(Self::from_params(&remote_network))),
			_ => None,
		}
	}
}
impl<UniversalLocation, AccountId> GlobalConsensusConvertsFor<UniversalLocation, AccountId> {
	fn from_params(network: &NetworkId) -> [u8; 32] {
		(b"glblcnsnss_", network).using_encoded(blake2_256)
	}
}

/// Converts a location which is a top-level parachain (i.e. a parachain held on a
/// Relay-chain which provides its own consensus) into a 32-byte `AccountId`.
///
/// This will always result in the *same account ID* being returned for the same
/// parachain index under the same Relay-chain, regardless of the relative security of
/// this Relay-chain compared to the local chain.
///
/// Note: No distinction is made when the local chain happens to be the parachain in
/// question or its Relay-chain.
///
/// WARNING: This results in the same `AccountId` value being generated regardless
/// of the relative security of the local chain and the Relay-chain of the input
/// location. This may not have any immediate security risks, however since it creates
/// commonalities between chains with different security characteristics, it could
/// possibly form part of a more sophisticated attack scenario.
pub struct GlobalConsensusParachainConvertsFor<UniversalLocation, AccountId>(
	PhantomData<(UniversalLocation, AccountId)>,
);
impl<UniversalLocation: Get<InteriorMultiLocation>, AccountId: From<[u8; 32]> + Clone>
	ConvertLocation<AccountId> for GlobalConsensusParachainConvertsFor<UniversalLocation, AccountId>
{
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		let universal_source = UniversalLocation::get();
		log::trace!(
			target: "xcm::location_conversion",
			"GlobalConsensusParachainConvertsFor universal_source: {:?}, location: {:?}",
			universal_source, location,
		);
		let devolved = ensure_is_remote(universal_source, *location).ok()?;
		let (remote_network, remote_location) = devolved;

		match remote_location {
			X1(Parachain(remote_network_para_id)) =>
				Some(AccountId::from(Self::from_params(&remote_network, &remote_network_para_id))),
			_ => None,
		}
	}
}
impl<UniversalLocation, AccountId>
	GlobalConsensusParachainConvertsFor<UniversalLocation, AccountId>
{
	fn from_params(network: &NetworkId, para_id: &u32) -> [u8; 32] {
		(b"glblcnsnss/prchn_", network, para_id).using_encoded(blake2_256)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	pub type ForeignChainAliasAccount<AccountId> =
		HashedDescription<AccountId, LegacyDescribeForeignChainAccount>;

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
	fn global_consensus_converts_for_works() {
		parameter_types! {
			pub UniversalLocationInNetwork1: InteriorMultiLocation = X2(GlobalConsensus(ByGenesis([1; 32])), Parachain(1234));
			pub UniversalLocationInNetwork2: InteriorMultiLocation = X2(GlobalConsensus(ByGenesis([2; 32])), Parachain(1234));
		}
		let network_1 = UniversalLocationInNetwork1::get().global_consensus().expect("NetworkId");
		let network_2 = UniversalLocationInNetwork2::get().global_consensus().expect("NetworkId");
		let network_3 = ByGenesis([3; 32]);
		let network_4 = ByGenesis([4; 32]);
		let network_5 = ByGenesis([5; 32]);

		let test_data = vec![
			(MultiLocation::parent(), false),
			(MultiLocation::new(0, Here), false),
			(MultiLocation::new(0, X1(GlobalConsensus(network_1))), false),
			(MultiLocation::new(1, X1(GlobalConsensus(network_1))), false),
			(MultiLocation::new(2, X1(GlobalConsensus(network_1))), false),
			(MultiLocation::new(0, X1(GlobalConsensus(network_2))), false),
			(MultiLocation::new(1, X1(GlobalConsensus(network_2))), false),
			(MultiLocation::new(2, X1(GlobalConsensus(network_2))), true),
			(MultiLocation::new(0, X2(GlobalConsensus(network_2), Parachain(1000))), false),
			(MultiLocation::new(1, X2(GlobalConsensus(network_2), Parachain(1000))), false),
			(MultiLocation::new(2, X2(GlobalConsensus(network_2), Parachain(1000))), false),
		];

		for (location, expected_result) in test_data {
			let result =
				GlobalConsensusConvertsFor::<UniversalLocationInNetwork1, [u8; 32]>::convert_location(
					&location,
				);
			match result {
				Some(account) => {
					assert_eq!(
						true, expected_result,
						"expected_result: {}, but conversion passed: {:?}, location: {:?}",
						expected_result, account, location
					);
					match &location {
						MultiLocation { interior: X1(GlobalConsensus(network)), .. } =>
							assert_eq!(
								account,
								GlobalConsensusConvertsFor::<UniversalLocationInNetwork1, [u8; 32]>::from_params(network),
								"expected_result: {}, but conversion passed: {:?}, location: {:?}", expected_result, account, location
							),
						_ => panic!("expected_result: {}, conversion passed: {:?}, but MultiLocation does not match expected pattern, location: {:?}", expected_result, account, location)
					}
				},
				None => {
					assert_eq!(
						false, expected_result,
						"expected_result: {} - but conversion failed, location: {:?}",
						expected_result, location
					);
				},
			}
		}

		// all success
		let res_1_gc_network_3 =
			GlobalConsensusConvertsFor::<UniversalLocationInNetwork1, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X1(GlobalConsensus(network_3))),
			)
			.expect("conversion is ok");
		let res_2_gc_network_3 =
			GlobalConsensusConvertsFor::<UniversalLocationInNetwork2, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X1(GlobalConsensus(network_3))),
			)
			.expect("conversion is ok");
		let res_1_gc_network_4 =
			GlobalConsensusConvertsFor::<UniversalLocationInNetwork1, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X1(GlobalConsensus(network_4))),
			)
			.expect("conversion is ok");
		let res_2_gc_network_4 =
			GlobalConsensusConvertsFor::<UniversalLocationInNetwork2, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X1(GlobalConsensus(network_4))),
			)
			.expect("conversion is ok");
		let res_1_gc_network_5 =
			GlobalConsensusConvertsFor::<UniversalLocationInNetwork1, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X1(GlobalConsensus(network_5))),
			)
			.expect("conversion is ok");
		let res_2_gc_network_5 =
			GlobalConsensusConvertsFor::<UniversalLocationInNetwork2, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X1(GlobalConsensus(network_5))),
			)
			.expect("conversion is ok");

		assert_ne!(res_1_gc_network_3, res_1_gc_network_4);
		assert_ne!(res_1_gc_network_4, res_1_gc_network_5);
		assert_ne!(res_1_gc_network_3, res_1_gc_network_5);

		assert_eq!(res_1_gc_network_3, res_2_gc_network_3);
		assert_eq!(res_1_gc_network_4, res_2_gc_network_4);
		assert_eq!(res_1_gc_network_5, res_2_gc_network_5);
	}

	#[test]
	fn global_consensus_parachain_converts_for_works() {
		parameter_types! {
			pub UniversalLocation: InteriorMultiLocation = X2(GlobalConsensus(ByGenesis([9; 32])), Parachain(1234));
		}

		let test_data = vec![
			(MultiLocation::parent(), false),
			(MultiLocation::new(0, X1(Parachain(1000))), false),
			(MultiLocation::new(1, X1(Parachain(1000))), false),
			(
				MultiLocation::new(
					2,
					X3(
						GlobalConsensus(ByGenesis([0; 32])),
						Parachain(1000),
						AccountId32 { network: None, id: [1; 32].into() },
					),
				),
				false,
			),
			(MultiLocation::new(2, X1(GlobalConsensus(ByGenesis([0; 32])))), false),
			(
				MultiLocation::new(0, X2(GlobalConsensus(ByGenesis([0; 32])), Parachain(1000))),
				false,
			),
			(
				MultiLocation::new(1, X2(GlobalConsensus(ByGenesis([0; 32])), Parachain(1000))),
				false,
			),
			(MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([0; 32])), Parachain(1000))), true),
			(
				MultiLocation::new(3, X2(GlobalConsensus(ByGenesis([0; 32])), Parachain(1000))),
				false,
			),
			(
				MultiLocation::new(9, X2(GlobalConsensus(ByGenesis([0; 32])), Parachain(1000))),
				false,
			),
		];

		for (location, expected_result) in test_data {
			let result =
				GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_location(
					&location,
				);
			match result {
				Some(account) => {
					assert_eq!(
						true, expected_result,
						"expected_result: {}, but conversion passed: {:?}, location: {:?}",
						expected_result, account, location
					);
					match &location {
						MultiLocation { interior: X2(GlobalConsensus(network), Parachain(para_id)), .. } =>
							assert_eq!(
								account,
								GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::from_params(network, para_id),
								"expected_result: {}, but conversion passed: {:?}, location: {:?}", expected_result, account, location
							),
						_ => assert_eq!(
							true,
							expected_result,
							"expected_result: {}, conversion passed: {:?}, but MultiLocation does not match expected pattern, location: {:?}", expected_result, account, location
						)
					}
				},
				None => {
					assert_eq!(
						false, expected_result,
						"expected_result: {} - but conversion failed, location: {:?}",
						expected_result, location
					);
				},
			}
		}

		// all success
		let res_gc_a_p1000 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([3; 32])), Parachain(1000))),
			)
			.expect("conversion is ok");
		let res_gc_a_p1001 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([3; 32])), Parachain(1001))),
			)
			.expect("conversion is ok");
		let res_gc_b_p1000 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([4; 32])), Parachain(1000))),
			)
			.expect("conversion is ok");
		let res_gc_b_p1001 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_location(
				&MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([4; 32])), Parachain(1001))),
			)
			.expect("conversion is ok");
		assert_ne!(res_gc_a_p1000, res_gc_a_p1001);
		assert_ne!(res_gc_a_p1000, res_gc_b_p1000);
		assert_ne!(res_gc_a_p1000, res_gc_b_p1001);
		assert_ne!(res_gc_b_p1000, res_gc_b_p1001);
		assert_ne!(res_gc_b_p1000, res_gc_a_p1001);
		assert_ne!(res_gc_b_p1001, res_gc_a_p1001);
	}

	#[test]
	fn remote_account_convert_on_para_sending_para_32() {
		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(1), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(2), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 1,
			interior: X2(Parachain(2), AccountKey20 { network: None, key: [0u8; 20] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 1,
			interior: X1(AccountId32 { network: None, id: [1u8; 32] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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
		let rem_1 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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

		assert_eq!(ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap(), rem_1);

		let mul = MultiLocation {
			parents: 0,
			interior: X2(Parachain(2), AccountId32 { network: None, id: [0u8; 32] }),
		};
		let rem_2 = ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).unwrap();

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
		assert!(ForeignChainAliasAccount::<[u8; 32]>::convert_location(&mul).is_none());
	}
}
