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

pub struct AliasForeignAccountId32<Prefix, Target>(PhantomData<(Prefix, Target)>);
impl<Prefix: Contains<MultiLocation>, Target: Contains<MultiLocation>>
	ContainsPair<MultiLocation, MultiLocation> for AliasForeignAccountId32<Prefix, Target>
{
	fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		if let (prefix, Some(account_id @ AccountId32 { .. })) = origin.split_last_interior() {
			return Prefix::contains(&prefix) &&
				Target::contains(target) &&
				Some(&account_id) == target.last()
		}
		false
	}
}

pub struct SiblingPrefix;
impl Contains<MultiLocation> for SiblingPrefix {
	fn contains(t: &MultiLocation) -> bool {
		match *t {
			MultiLocation { parents: 1, interior: X1(Parachain(_)) } => true,
			_ => false,
		}
	}
}

pub struct ParentPrefix;
impl Contains<MultiLocation> for ParentPrefix {
	fn contains(t: &MultiLocation) -> bool {
		match *t {
			MultiLocation { parents: 1, interior: Here } => true,
			_ => false,
		}
	}
}

pub struct ChildPrefix;
impl Contains<MultiLocation> for ChildPrefix {
	fn contains(t: &MultiLocation) -> bool {
		match *t {
			MultiLocation { parents: 0, interior: X1(Parachain(_)) } => true,
			_ => false,
		}
	}
}

pub struct IsNativeAccountId32;
impl Contains<MultiLocation> for IsNativeAccountId32 {
	fn contains(t: &MultiLocation) -> bool {
		match *t {
			MultiLocation { parents: 0, interior: X1(AccountId32 { .. }) } => true,
			_ => false,
		}
	}
}