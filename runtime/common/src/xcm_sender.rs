// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! XCM sender for relay chain.

use parity_scale_codec::Encode;
use runtime_parachains::{configuration, dmp};
use sp_std::marker::PhantomData;
use xcm::latest::prelude::*;

/// XCM sender for relay chain. It only sends downward message.
pub struct ChildParachainRouter<T, W>(PhantomData<(T, W)>);

impl<T: configuration::Config + dmp::Config, W: xcm::WrapVersion> SendXcm
	for ChildParachainRouter<T, W>
{
	fn send_xcm(dest: MultiLocation, msg: Xcm<()>) -> SendResult {
		match dest {
			MultiLocation { parents: 0, interior: X1(Parachain(id)) } => {
				// Downward message passing.
				let versioned_xcm =
					W::wrap_version(&dest, msg).map_err(|()| SendError::DestinationUnsupported)?;
				let config = <configuration::Pallet<T>>::config();
				<dmp::Pallet<T>>::queue_downward_message(
					&config,
					id.into(),
					versioned_xcm.encode(),
				)
				.map_err(Into::<SendError>::into)?;
				Ok(())
			},
			dest => Err(SendError::CannotReachDestination(dest, msg)),
		}
	}
}
