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
use sp_std::result::Result;
use xcm::latest::{prelude::*, Weight};

/// Determine the weight of an XCM message.
pub trait WeightBounds<RuntimeCall> {
	/// Return the maximum amount of weight that an attempted execution of this message could
	/// consume.
	fn weight(message: &mut Xcm<RuntimeCall>) -> Result<Weight, ()>;

	/// Return the maximum amount of weight that an attempted execution of this instruction could
	/// consume.
	fn instr_weight(instruction: &Instruction<RuntimeCall>) -> Result<Weight, ()>;
}

/// A means of getting approximate weight consumption for a given destination message executor and a
/// message.
pub trait UniversalWeigher {
	/// Get the upper limit of weight required for `dest` to execute `message`.
	fn weigh(dest: impl Into<MultiLocation>, message: Xcm<()>) -> Result<Weight, ()>;
}

/// Charge for weight in order to execute XCM.
///
/// A `WeightTrader` may also be put into a tuple, in which case the default behavior of
/// `buy_weight` and `refund_weight` would be to attempt to call each tuple element's own
/// implementation of these two functions, in the order of which they appear in the tuple,
/// returning early when a successful result is returned.
pub trait WeightTrader: Sized {
	/// Create a new trader instance.
	fn new() -> Self;

	/// Purchase execution weight credit in return for up to a given `payment`. If less of the
	/// payment is required then the surplus is returned. If the `payment` cannot be used to pay
	/// for the `weight`, then an error is returned.
	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, XcmError>;

	/// Attempt a refund of `weight` into some asset. The caller does not guarantee that the weight was
	/// purchased using `buy_weight`.
	///
	/// Default implementation refunds nothing.
	fn refund_weight(&mut self, _weight: Weight) -> Option<MultiAsset> {
		None
	}
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl WeightTrader for Tuple {
	fn new() -> Self {
		for_tuples!( ( #( Tuple::new() ),* ) )
	}

	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, XcmError> {
		let mut last_error = None;
		for_tuples!( #(
			match Tuple.buy_weight(weight, payment.clone()) {
				Ok(assets) => return Ok(assets),
				Err(e) => { last_error = Some(e) }
			}
		)* );
		let last_error = last_error.unwrap_or(XcmError::TooExpensive);
		log::trace!(target: "xcm::buy_weight", "last_error: {:?}", last_error);
		Err(last_error)
	}

	fn refund_weight(&mut self, weight: Weight) -> Option<MultiAsset> {
		for_tuples!( #(
			if let Some(asset) = Tuple.refund_weight(weight) {
				return Some(asset);
			}
		)* );
		None
	}
}
