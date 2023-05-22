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

use frame_support::traits::{ContainsPair, Get};
use sp_std::marker::PhantomData;
use xcm::latest::prelude::*;

pub struct RemovePrefixAccountId32<Prefix>(PhantomData<Prefix>);
impl<Prefix: Get<MultiLocation>> ContainsPair<MultiLocation, MultiLocation>
	for RemovePrefixAccountId32<Prefix>
{
	fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		if let Ok(appended) = (*target).clone().prepended_with(Prefix::get()) {
			if appended == *origin {
				match appended.last() {
					Some(AccountId32 { .. }) => return true,
					_ => return false,
				}
			}
		}
		false
	}
}

pub struct AliasCase<T>(PhantomData<T>);
impl<T: Get<(MultiLocation, MultiLocation)>> ContainsPair<MultiLocation, MultiLocation>
	for AliasCase<T>
{
	fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		let (o, t) = T::get();
		&o == origin && &t == target
	}
}
