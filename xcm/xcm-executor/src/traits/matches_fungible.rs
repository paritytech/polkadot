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

use xcm::v0::MultiAsset;

pub trait MatchesFungible<Balance> {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance>;
}

// TODO: #MATCHTUPLE impl for tuples
impl<B: From<u128>, X: MatchesFungible<B>, Y: MatchesFungible<B>> MatchesFungible<B> for (X, Y) {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		X::matches_fungible(a).or_else(|| Y::matches_fungible(a))
	}
}
