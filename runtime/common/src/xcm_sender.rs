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
use primitives::v1::Id as ParaId;
use runtime_parachains::{configuration::{self, HostConfiguration}, dmp};
use sp_std::{prelude::*, marker::PhantomData};
use xcm::prelude::*;
use SendError::*;

/// XCM sender for relay chain. It only sends downward message.
pub struct ChildParachainRouter<T, W>(PhantomData<(T, W)>);

impl<T: configuration::Config + dmp::Config, W: xcm::WrapVersion> SendXcm
	for ChildParachainRouter<T, W>
{
	type OptionTicket = Option<(HostConfiguration<T::BlockNumber>, ParaId, Vec<u8>)>;

	fn validate(dest: &mut Option<MultiLocation>, msg: &mut Option<Xcm<()>>) -> SendResult<(HostConfiguration<T::BlockNumber>, ParaId, Vec<u8>)> {
		let d = dest.take().ok_or(MissingArgument)?;
		let id = if let MultiLocation { parents: 0, interior: X1(Parachain(id)) } = &d {
			*id
		} else {
			*dest = Some(d);
			return Err(CannotReachDestination)
		};

		// Downward message passing.
		let xcm = msg.take().ok_or(MissingArgument)?;
		let config = <configuration::Pallet<T>>::config();
		let para = id.into();
		let blob = W::wrap_version(&d, xcm).map_err(|()| DestinationUnsupported)?.encode();
		<dmp::Pallet<T>>::can_queue_downward_message(&config, &para, &blob)
			.map_err(Into::<SendError>::into)?;

		Ok(((config, para, blob), MultiAssets::new()))
	}

	fn deliver((config, para, blob): (HostConfiguration<T::BlockNumber>, ParaId, Vec<u8>)) -> Result<(), SendError> {
		<dmp::Pallet<T>>::queue_downward_message(&config, para, blob)
			.map_err(|_| SendError::Transport(&"Error placing into DMP queue"))
	}
}
