// Copyright (C) Parity Technologies (UK) Ltd.
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

use frame_support::traits::{Contains, ContainsPair};
use sp_std::marker::PhantomData;
use xcm::latest::prelude::*;

pub struct AliasCase<Origin, Target>(PhantomData<(Origin, Target)>);
impl<Origin, Target> ContainsPair<MultiLocation, MultiLocation> for AliasCase<Origin, Target>
where
	Origin: Contains<MultiLocation>,
	Target: Contains<MultiLocation>,
{
	fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		Origin::contains(origin) && Target::contains(target)
	}
}

pub struct AliasForeignAccountId32<Prefix>(PhantomData<Prefix>);
impl<Prefix: Contains<MultiLocation>> ContainsPair<MultiLocation, MultiLocation>
	for AliasForeignAccountId32<Prefix>
{
	fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		if let (prefix, Some(account_id @ AccountId32 { .. })) = origin.split_last_interior() {
			return Prefix::contains(&prefix) &&
				*target == MultiLocation { parents: 0, interior: X1(account_id) }
		}
		false
	}
}
