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

use xcm::latest::prelude::*;

/// Type which is able to send a given message over to another consensus network.
pub trait ExportXcm {
	fn export_price(
		_network: NetworkId,
		_destination: &InteriorMultiLocation,
		_message: &Xcm<()>,
	) -> SendCostResult {
		Ok(MultiAssets::new())
	}

	fn export_xcm(
		network: NetworkId,
		channel: u32,
		destination: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ExportXcm for Tuple {
	fn export_price(
		network: NetworkId,
		dest: &InteriorMultiLocation,
		message: &Xcm<()>,
	) -> SendCostResult {
		for_tuples!( #(
			match Tuple::export_price(network, dest, message) {
				Err(SendError::CannotReachDestination) => {},
				o @ _ => return o,
			};
		)* );
		Err(SendError::CannotReachDestination)
	}

	fn export_xcm(
		network: NetworkId,
		channel: u32,
		dest: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult {
		for_tuples!( #(
			match Tuple::export_xcm(network, channel, dest, message) {
				Err(SendError::CannotReachDestination) => {},
				o @ _ => return o,
			};
		)* );
		Err(SendError::CannotReachDestination)
	}
}

/// Convenience function for using a `SendXcm` implementation. Just interprets the `dest` and wraps
/// both in `Some` before passing them as as mutable references into `T::send_xcm`.
pub fn export_xcm<T: ExportXcm>(
	network: NetworkId,
	channel: u32,
	dest: InteriorMultiLocation,
	message: Xcm<()>,
) -> SendResult {
	T::export_xcm(network, channel, &mut Some(dest), &mut Some(message))
}
