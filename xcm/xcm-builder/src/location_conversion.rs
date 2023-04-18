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
use parity_scale_codec::{Decode, Encode};
use sp_io::hashing::blake2_256;
use sp_runtime::traits::{AccountIdConversion, TrailingZeroInput};
use sp_std::{borrow::Borrow, marker::PhantomData};
use xcm::latest::prelude::*;
use xcm_executor::traits::Convert;

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
	Convert<MultiLocation, AccountId>
	for GlobalConsensusParachainConvertsFor<UniversalLocation, AccountId>
{
	fn convert_ref(location: impl Borrow<MultiLocation>) -> Result<AccountId, ()> {
		let universal_source = UniversalLocation::get();
		log::trace!(
			target: "xcm::location_conversion",
			"GlobalConsensusParachainConvertsFor universal_source: {:?}, location: {:?}",
			universal_source, location.borrow(),
		);
		let devolved = ensure_is_remote(universal_source, *location.borrow()).map_err(|_| ())?;
		let (remote_network, remote_location) = devolved;

		match remote_location {
			X1(Parachain(remote_network_para_id)) =>
				Ok(AccountId::from(GlobalConsensusParachainConvertsFor::<
					UniversalLocation,
					AccountId,
				>::from_params(&remote_network, &remote_network_para_id))),
			_ => Err(()),
		}
	}

	fn reverse_ref(_: impl Borrow<AccountId>) -> Result<MultiLocation, ()> {
		// if this will be needed, we could implement some kind of guessing, if we have configuration for supported networkId+paraId
		Err(())
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
				GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_ref(
					&location,
				);
			match result {
				Ok(account) => {
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
				Err(_) => {
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
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_ref(
				MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([3; 32])), Parachain(1000))),
			)
			.expect("conversion is ok");
		let res_gc_a_p1001 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_ref(
				MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([3; 32])), Parachain(1001))),
			)
			.expect("conversion is ok");
		let res_gc_b_p1000 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_ref(
				MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([4; 32])), Parachain(1000))),
			)
			.expect("conversion is ok");
		let res_gc_b_p1001 =
			GlobalConsensusParachainConvertsFor::<UniversalLocation, [u8; 32]>::convert_ref(
				MultiLocation::new(2, X2(GlobalConsensus(ByGenesis([4; 32])), Parachain(1001))),
			)
			.expect("conversion is ok");
		assert_ne!(res_gc_a_p1000, res_gc_a_p1001);
		assert_ne!(res_gc_a_p1000, res_gc_b_p1000);
		assert_ne!(res_gc_a_p1000, res_gc_b_p1001);
		assert_ne!(res_gc_b_p1000, res_gc_b_p1001);
		assert_ne!(res_gc_b_p1000, res_gc_a_p1001);
		assert_ne!(res_gc_b_p1001, res_gc_a_p1001);
	}
}
