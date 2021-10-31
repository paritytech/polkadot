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
	fn export_xcm(
		network: NetworkId,
		channel: u32,
		destination: impl Into<InteriorMultiLocation>,
		message: Xcm<()>,
	) -> Result<(), SendError>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ExportXcm for Tuple {
	fn export_xcm(
		network: NetworkId,
		channel: u32,
		dest: impl Into<InteriorMultiLocation>,
		message: Xcm<()>,
	) -> SendResult {
		let dest = dest.into();
		for_tuples!( #(
			// we shadow `dest` and `message` in each expansion for the next one.
			let (network, dest, message) = match Tuple::export_xcm(network, channel, dest, message) {
				Err(SendError::CannotReachNetwork(n, d, m)) => (n, d, m),
				o @ _ => return o,
			};
		)* );
		Err(SendError::CannotReachNetwork(network, dest, message))
	}
}
