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

use sp_std::result::Result;
use xcm::latest::{MultiLocation, Weight, Xcm};

/// Trait to determine whether the execution engine should actually execute a given XCM.
///
/// Can be amalgamated into a tuple to have multiple trials. If any of the tuple elements returns `Ok()`, the
/// execution stops. Else, `Err(_)` is returned if all elements reject the message.
pub trait ShouldExecute {
	/// Returns `true` if the given `message` may be executed.
	///
	/// - `origin`: The origin (sender) of the message.
	/// - `message`: The message itself.
	/// - `max_weight`: The (possibly over-) estimation of the weight of execution of the message.
	/// - `weight_credit`: The pre-established amount of weight that the system has determined this
	///   message may utilize in its execution. Typically non-zero only because of prior fee
	///   payment, but could in principle be due to other factors.
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		message: &mut Xcm<RuntimeCall>,
		max_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ShouldExecute for Tuple {
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		message: &mut Xcm<RuntimeCall>,
		max_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		for_tuples!( #(
			match Tuple::should_execute(origin, message, max_weight, weight_credit) {
				Ok(()) => return Ok(()),
				_ => (),
			}
		)* );
		log::trace!(
			target: "xcm::should_execute",
			"did not pass barrier: origin: {:?}, message: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin,
			message,
			max_weight,
			weight_credit,
		);
		Err(())
	}
}
