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

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::convert::TryInto;
use frame_support::dispatch::Dispatchable;
use codec::{Encode, Decode};
use xcm::{VersionedXcm, v0::{
	Xcm, Ais, Ai, SendXcm, ExecuteXcm, XcmError, XcmResult,
	MultiOrigin, MultiAssets, MultiAsset, AssetInstance, MultiLocation, Junction,
}};

mod traits;
mod assets;
mod config;
mod currency_adapter;

use traits::TransactAsset;
pub use assets::{Assets, AssetId};
pub use config::Config;
pub use currency_adapter::CurrencyAdapter;
// TODO: pub use multiasset_adapter::MultiAssetAdapter;
use sp_std::marker::PhantomData;

pub struct XcmExecutor<Config>(PhantomData<Config>);

impl<Config: config::Config> ExecuteXcm for XcmExecutor<Config> {
	fn execute_xcm(origin: MultiLocation, msg: Xcm) -> XcmResult {
		let (mut holding, effects) = match (origin, msg) {
			(origin, Xcm::ForwardedFromParachain { id, inner }) => {
				let new_origin = origin.pushed_with(Junction::Parachain { id }).map_err(|_| ())?;
				Self::execute_xcm(new_origin, (*inner).try_into()?)
			}
			(_origin, Xcm::WithdrawAsset { assets, effects }) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let mut holding = Assets::default();
				for asset in assets {
					let withdrawn = Config::AssetTransactor::withdraw_asset(&asset)?;
					holding.saturating_subsume(withdrawn);
				}
				(holding, effects)
			}
			(origin, Xcm::ReserveAssetCredit { assets, effects }) => {
				// TODO: check whether we trust origin to be our reserve location for this asset via
				//   config trait.
				if assets.len() == 1 &&
					matches!(&assets[0], MultiAsset::ConcreteFungible { ref id, .. } if id == &origin)
				{
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					(Assets::from(assets), effects)
				} else {
					Err(())?
				}
			}
			(_origin, Xcm::TeleportAsset { assets, effects }) => {
				// TODO: check whether we trust origin to teleport this asset to us via config trait.
				Err(())?	// << we don't trust any chains, for now.
			}
			(origin, Xcm::Transact { origin_type, call }) => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.

				// TODO: Weight fees should be paid.

				// TODO: allow this to be configurable in the trait.
				// TODO: allow the trait to issue filters for the relay-chain
				if let Ok(message_call) = Config::Call::decode(&mut &call[..]) {
					let dispatch_origin = Config::OriginConverter::convert((origin_type, origin))?;
					let _ok = message_call.dispatch(dispatch_origin).is_ok();
					// Not much to do with the result as it is. It's up to the parachain to ensure that the
					// message makes sense.
					return Ok(());
				}
			}
			_ => Err(())?,	// Unhandled XCM message.
		};

		// TODO: stuff that should happen after holding is populated but before effects,
		//   including depositing fees for effects from holding account.

		for effect in effects.into_iter() {
			let _ = Self::execute_effects(&origin, &mut holding, effect)?;
		}

		// TODO: stuff that should happen after effects including refunding unused fees.

		Ok(())
	}
}

impl<Config: config::Config> XcmExecutor<Config> {
	fn execute_effects(origin: &MultiLocation, holding: &mut Assets, effect: Ai) -> XcmResult {
		match effect {
			Ai::DepositAsset { assets, dest } => {
				let deposited = holding.saturating_take(assets);
				for (id, amount) in deposited.fungible.into_iter() {
					// TODO: extract into a From impl
					let asset = match id {
						AssetId::Concrete(id) => MultiAsset::ConcreteFungible { id, amount },
						AssetId::Abstract(id) => MultiAsset::AbstractFungible { id, amount },
					};
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				for (id, instance) in deposited.non_fungible {
					// TODO: extract into a From impl
					let asset = match id {
						AssetId::Concrete(class) => MultiAsset::ConcreteNonFungible { class, instance },
						AssetId::Abstract(class) => MultiAsset::AbstractNonFungible { class, instance },
					};
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
			},
			_ => Err(()),
		}
		Ok(())
	}
}

// Example only - move into test and/or runtimes.
/*
parameter_types! {
	const DotLocation: MultiLocation = MultiLocation::X1(Junction::Parent);
	const DotName: &'static [u8] = &b"DOT"[..];
	const MyLocation: MultiLocation = MultiLocation::Null;
	const MyName: &'static [u8] = &b"ABC"[..];
}
type MyDepositAsset = (
	// Convert a Currency impl into a DepositAsset
	CurrencyAdapter<
		// Use this currency:
		balances_pallet::Module::<T, Instance1>,
		// Use this currency when it is a fungible asset matching the given location or name:
		(IsConcrete<DotLocation>, IsAbstract<DotName>),
		// Do a simple punn to convert an AccountId32 MultiLocation into a native chain account ID:
		AccountId32Punner<T::AccountId>,
		// Our chain's account ID type (we can't get away without mentioning it explicitly):
		T::AccountId,
	>,
	CurrencyAdapter<
		balances_pallet::Module::<T, DefaultInstance>,
		(IsConcrete<MyLocation>, IsAbstract<MyName>),
		AccountId32Punner<T::AccountId>,
		T::AccountId,
	>,
);
*/
