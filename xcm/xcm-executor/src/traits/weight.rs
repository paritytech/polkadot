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
use xcm::v0::{Error, MultiAsset, MultiLocation, Xcm};

/// Determine the weight of an XCM message.
pub trait WeightBounds<Call> {
	/// Return the minimum amount of weight that an attempted execution of this message would definitely
	/// consume.
	///
	/// This is useful to gauge how many fees should be paid up front to begin execution of the message.
	/// It is not useful for determining whether execution should begin lest it result in surpassing weight
	/// limits - in that case `deep` is the function to use.
	fn shallow(message: &mut Xcm<Call>) -> Result<Weight, ()>;

	/// Return the deep amount of weight, over `shallow` that complete, successful and worst-case execution of
	/// `message` would incur.
	///
	/// This is perhaps overly pessimistic for determining how many fees should be paid for up-front since
	/// fee payment (or any other way of offsetting the execution costs such as an voucher-style NFT) may
	/// happen in stages throughout execution of the XCM.
	///
	/// A reminder: if it is possible that `message` may have alternative means of successful completion
	/// (perhaps a conditional path), then the *worst case* weight must be reported.
	///
	/// This is guaranteed equal to the eventual sum of all `shallow` XCM messages that get executed through
	/// any internal effects. Inner XCM messages may be executed by:
	/// - `Order::BuyExecution`
	fn deep(message: &mut Xcm<Call>) -> Result<Weight, ()>;

	/// Return the total weight for executing `message`.
	fn weight(message: &mut Xcm<Call>) -> Result<Weight, ()> {
		Self::shallow(message)?.checked_add(Self::deep(message)?).ok_or(())
	}
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
