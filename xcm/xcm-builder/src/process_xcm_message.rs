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

//! Implementation of `ProcessMessage` for an `ExecuteXcm` implementation.

use sp_std::{fmt::Debug, marker::PhantomData};
use sp_io::hashing::blake2_256;
use sp_weights::Weight;
use parity_scale_codec::{Decode, FullCodec, MaxEncodedLen};
use scale_info::TypeInfo;
use frame_support::{ensure, traits::{ProcessMessage, ProcessMessageError}};
use xcm::prelude::*;

pub struct ProcessXcmMessage<MessageOrigin, XcmExecutor, Call>(PhantomData<(MessageOrigin, XcmExecutor, Call)>);
impl<
	MessageOrigin: Into<MultiLocation> + FullCodec + MaxEncodedLen + Clone + Eq + PartialEq + TypeInfo + Debug,
	XcmExecutor: ExecuteXcm<Call>,
	Call,
> ProcessMessage for ProcessXcmMessage<MessageOrigin, XcmExecutor, Call> {
	type Origin = MessageOrigin;

	/// Process the given message, using no more than `weight_limit` in weight to do so.
	fn process_message(
		message: &[u8],
		origin: Self::Origin,
		weight_limit: Weight,
	) -> Result<(bool, Weight), ProcessMessageError> {
		let hash = blake2_256(message);
		let versioned_message = VersionedXcm::<Call>::decode(&mut &message[..])
			.map_err(|_| ProcessMessageError::Corrupt)?;
		let message = Xcm::<Call>::try_from(versioned_message)
			.map_err(|_| ProcessMessageError::Unsupported)?;
		let pre = XcmExecutor::prepare(message).map_err(|_| ProcessMessageError::Unsupported)?;
		let weight = Weight::from_ref_time(pre.weight_of());
		ensure!(weight.all_lte(weight_limit), ProcessMessageError::Overweight(weight));
		match XcmExecutor::execute(origin.into(), pre, hash, 0) {
			Outcome::Complete(w) => {
				Ok((true, Weight::from_ref_time(w)))
			},
			Outcome::Incomplete(w, _) => {
				Ok((false, Weight::from_ref_time(w)))
			},
			Outcome::Error(_) => {
				Err(ProcessMessageError::Unsupported)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn process_message_works() {

	}
}