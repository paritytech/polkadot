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

use sp_std::{prelude::*, marker::PhantomData, convert::TryInto};
use frame_support::{ensure, dispatch::Dispatchable};
use codec::Decode;
use xcm::v0::{Xcm, Ai, ExecuteXcm, SendXcm, Result as XcmResult, MultiLocation, Junction};

pub mod traits;
mod assets;
mod config;

use traits::{TransactAsset, ConvertOrigin, FilterAssetLocation, InvertLocation};
pub use assets::{Assets, AssetId};
pub use config::Config;

pub struct XcmExecutor<Config>(PhantomData<Config>);

impl<Config: config::Config> ExecuteXcm for XcmExecutor<Config> {
	fn execute_xcm(origin: MultiLocation, msg: Xcm) -> XcmResult {
		let (mut holding, effects) = match (origin.clone(), msg) {
			(origin, Xcm::RelayedFrom { superorigin, inner }) => {
				// We ensure that it doesn't contain any `Parent` Junctions which would imply a privilege escalation.
				let mut new_origin = origin;
				for j in superorigin.into_iter() {
					ensure!(j.is_sub_consensus(), ());
					new_origin.push(j).map_err(|_| ())?;
				}
				return Self::execute_xcm(new_origin, (*inner).try_into()?)
			}
			(origin, Xcm::WithdrawAsset { assets, effects }) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let mut holding = Assets::default();
				for asset in assets {
					let withdrawn = Config::AssetTransactor::withdraw_asset(&asset, &origin)?;
					holding.saturating_subsume(withdrawn);
				}
				(holding, effects)
			}
			(origin, Xcm::ReserveAssetCredit { assets, effects }) => {
				// check whether we trust origin to be our reserve location for this asset.
				if assets.iter().all(|asset| Config::IsReserve::filter_asset_location(asset, &origin)) {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					(Assets::from(assets), effects)
				} else {
					Err(())?
				}
			}
			(origin, Xcm::TeleportAsset { assets, effects }) => {
				// check whether we trust origin to teleport this asset to us via config trait.
				if assets.iter().all(|asset| Config::IsTeleporter::filter_asset_location(asset, &origin)) {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					(Assets::from(assets), effects)
				} else {
					Err(())?
				}
			}
			(origin, Xcm::Transact { origin_type, call }) => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.

				// TODO: Weight fees should be paid.

				// TODO: allow this to be configurable in the trait.
				// TODO: allow the trait to issue filters for the relay-chain
				let message_call = Config::Call::decode(&mut &call[..]).map_err(|_| ())?;
				let dispatch_origin = Config::OriginConverter::convert_origin(origin, origin_type)
					.map_err(|_| ())?;
				let _ok = message_call.dispatch(dispatch_origin).is_ok();
				// Not much to do with the result as it is. It's up to the parachain to ensure that the
				// message makes sense.
				return Ok(());
			}
			(origin, Xcm::RelayToParachain { id, inner }) => {
				let msg = Xcm::RelayedFrom { superorigin: origin, inner }.into();
				return Config::XcmSender::send_xcm(Junction::Parachain { id }.into(), msg)
			},
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
	fn execute_effects(_origin: &MultiLocation, holding: &mut Assets, effect: Ai) -> XcmResult {
		match effect {
			Ai::DepositAsset { assets, dest } => {
				let deposited = holding.saturating_take(assets);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				Ok(())
			},
			Ai::DepositReserveAsset { assets, dest, effects } => {
				let mut deposited = holding.saturating_take(assets);

				for asset in deposited.clone().into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}

				let inv_dest = Config::LocationInverter::invert_location(&dest);
				deposited.reanchor(&inv_dest);
				let assets = deposited.into_assets_iter().collect::<Vec<_>>();

				let msg = Xcm::ReserveAssetCredit { assets, effects }.into();
				Config::XcmSender::send_xcm(dest, msg)
			},
			Ai::InitiateReserveTransfer { assets, reserve, dest, effects} => {
				let mut transferred = holding.saturating_take(assets);
				let inv_reserve = Config::LocationInverter::invert_location(&reserve);
				transferred.reanchor(&inv_reserve);
				let assets = transferred.into_assets_iter().collect::<Vec<_>>();
				let msg = Xcm::WithdrawAsset { assets: assets.clone(), effects: vec![
					Ai::DepositReserveAsset { assets, dest, effects }
				] };
				Config::XcmSender::send_xcm(reserve, msg)
			}
			Ai::InitiateTeleport { assets, dest, effects} => {
				let transferred = holding.saturating_take(assets);
				let assets = transferred.into_assets_iter().collect();
				Config::XcmSender::send_xcm(dest, Xcm::TeleportAsset { assets, effects })
			}
			Ai::QueryHolding { query_id, dest, assets } => {
				let assets = holding.min(assets.iter()).into_assets_iter().collect();
				Config::XcmSender::send_xcm(dest, Xcm::Balances { query_id, assets })
			}
			_ => Err(())?,
		}
	}
}
