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

use xcm::latest::MultiAsset;

pub trait MatchesFungible<Balance> {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<Balance> MatchesFungible<Balance> for Tuple {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance> {
		for_tuples!( #(
			match Tuple::matches_fungible(a) { o @ Some(_) => return o, _ => () }
		)* );
		log::trace!(target: "xcm::matches_fungible", "did not match fungible asset: {:?}", &a);
		None
	}
}
