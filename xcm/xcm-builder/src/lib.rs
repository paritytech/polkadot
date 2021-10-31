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

//! # XCM-Builder
//!
//! Types and helpers for *building* XCM configuration.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

mod location_conversion;
pub use location_conversion::{
	Account32Hash, AccountId32Aliases, AccountKey20Aliases, ChildParachainConvertsVia,
	LocationInverter, ParentIsDefault, SiblingParachainConvertsVia,
};

mod origin_conversion;
pub use origin_conversion::{
	BackingToPlurality, ChildParachainAsNative, ChildSystemParachainAsSuperuser, EnsureXcmOrigin,
	ParentAsSuperuser, RelayChainAsNative, SiblingParachainAsNative,
	SiblingSystemParachainAsSuperuser, SignedAccountId32AsNative, SignedAccountKey20AsNative,
	SignedToAccountId32, SovereignSignedViaLocation,
};

mod barriers;
pub use barriers::{
	AllowKnownQueryResponses, AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom,
	AllowUnpaidExecutionFrom, IsChildSystemParachain, TakeWeightCredit,
};

mod currency_adapter;
pub use currency_adapter::CurrencyAdapter;

mod fungibles_adapter;
pub use fungibles_adapter::{
	AsPrefixedGeneralIndex, ConvertedAbstractAssetId, ConvertedConcreteAssetId, FungiblesAdapter,
	FungiblesMutateAdapter, FungiblesTransferAdapter,
};

mod weight;
#[allow(deprecated)]
pub use weight::FixedRateOfConcreteFungible;
pub use weight::{
	FixedRateOfFungible, FixedWeightBounds, TakeRevenue, UsingComponents, WeightInfoBounds,
};

mod matches_fungible;
pub use matches_fungible::{IsAbstract, IsConcrete};

mod filter_asset_location;
pub use filter_asset_location::{Case, NativeAsset};

mod universal_exports {
	use sp_std::{prelude::*, marker::PhantomData, convert::TryInto};
	use frame_support::traits::Get;
	use xcm::prelude::*;
	use xcm_executor::traits::ExportXcm;

	fn ensure_is_remote(
		ancestry: impl Into<InteriorMultiLocation>,
		dest: impl Into<MultiLocation>,
	) -> Result<(NetworkId, InteriorMultiLocation), MultiLocation> {
		let dest = dest.into();
		let ancestry = ancestry.into();
		let local_network = match ancestry.first() {
			Some(GlobalConsensus(network)) => network.clone(),
			_ => return Err(dest),
		};
		let universal_destination: InteriorMultiLocation = ancestry
			.into_location()
			.appended_with(dest.clone())
			.map_err(|x| x.1)?
			.try_into()?;
		let (remote_dest, remote_net) = match universal_destination.split_first() {
			(d, Some(GlobalConsensus(n))) if n != local_network => (d, n),
			_ => return Err(dest),
		};
		Ok((remote_net, remote_dest))
	}

	pub struct LocalExporter<Exporter, Ancestry>(PhantomData<(Exporter, Ancestry)>);
	impl<Exporter: ExportXcm, Ancestry: Get<InteriorMultiLocation>> SendXcm for LocalExporter<Exporter, Ancestry> {
		fn send_xcm(dest: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
			let (network, destination) = match ensure_is_remote(Ancestry::get(), dest) {
				Ok(x) => x,
				Err(dest) => return Err(SendError::CannotReachDestination(dest, message)),
			};
			Exporter::export_xcm(network, 0, destination, message)
		}
	}

	#[test]
	fn local_exporter_works() {
		// A Kusama parachain is remote from the Polkadot Relay.
		let x = ensure_is_remote(Polkadot, (Parent, Kusama, Parachain(1000)));
		assert_eq!(x, Ok((Kusama, Parachain(1000).into())));

		// Polkadot Relay is remote from a Kusama parachain.
		let x = ensure_is_remote((Kusama, Parachain(1000)), (Parent, Parent, Polkadot));
		assert_eq!(x, Ok((Polkadot, ().into())));

		// Our own parachain is local.
		let x = ensure_is_remote(Polkadot, Parachain(1000));
		assert_eq!(x, Err(Parachain(1000).into()));

		// Polkadot's parachain is not remote if we are Polkadot.
		let x = ensure_is_remote(Polkadot, (Parent, Polkadot, Parachain(1000)));
		assert_eq!(x, Err((Parent, Polkadot, Parachain(1000)).into()));

		// If we don't have a consensus ancestor, then we cannot determine remoteness.
		let x = ensure_is_remote((), (Parent, Polkadot, Parachain(1000)));
		assert_eq!(x, Err((Parent, Polkadot, Parachain(1000)).into()));
	}
}
