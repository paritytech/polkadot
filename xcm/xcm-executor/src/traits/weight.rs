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

use crate::Assets;
use frame_support::weights::Weight;
use sp_std::result::Result;
use xcm::latest::{Error, MultiAsset, MultiLocation, Xcm};

/// Determine the weight of an XCM message.
pub trait WeightBounds<Call> {
	/// Return the maximum amount of weight that an attempted execution of this message could
	/// consume.
	fn weight(message: &mut Xcm<Call>) -> Result<Weight, ()>;
}

/// A means of getting approximate weight consumption for a given destination message executor and a
/// message.
pub trait UniversalWeigher {
	/// Get the upper limit of weight required for `dest` to execute `message`.
	fn weigh(dest: MultiLocation, message: Xcm<()>) -> Result<Weight, ()>;
}

/// Charge for weight in order to execute XCM.
pub trait WeightTrader: Sized {
	/// Create a new trader instance.
	fn new() -> Self;

	/// Purchase execution weight credit in return for up to a given `fee`. If less of the fee is required
	/// then the surplus is returned. If the `fee` cannot be used to pay for the `weight`, then an error is
	/// returned.
	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, Error>;

	/// Attempt a refund of `weight` into some asset. The caller does not guarantee that the weight was
	/// purchased using `buy_weight`.
	///
	/// Default implementation refunds nothing.
	fn refund_weight(&mut self, _weight: Weight) -> Option<MultiAsset> {
		None
	}
}

impl WeightTrader for () {
	fn new() -> Self {
		()
	}
	fn buy_weight(&mut self, _: Weight, _: Assets) -> Result<Assets, Error> {
		Err(Error::Unimplemented)
	}
}
