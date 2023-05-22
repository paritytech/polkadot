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
use xcm::latest::prelude::*;

pub struct AliasSiblingAccountId32<ParachainId>(sp_std::marker::PhantomData<ParachainId>);
impl<ParachainId: Get<u32>> ContainsPair<MultiLocation, MultiLocation>
	for AliasSiblingAccountId32<ParachainId>
{
	fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		match origin {
			MultiLocation {
				parents: 1,
				interior:
					X2(Parachain(para_id), origin_account  @  AccountId32 {network: _, id: _, }),
			} if *para_id == ParachainId::get() => match target {
				MultiLocation {
					parents: 0,
					interior: X1(alias_account),
				} if origin_account == alias_account => true,
				_ => false,
			},
			_ => false,
		}
	}
}

pub struct AliasParentAccountId32;
impl ContainsPair<MultiLocation, MultiLocation> for AliasParentAccountId32 {
    fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
        match origin {
            MultiLocation {
                parents: 1,
                interior: X1(origin_account  @  AccountId32 {network: _, id: _, })
            } => match target {
                MultiLocation {
                    parents: 0,
                    interior: X1(alias_account)
                } if alias_account == origin_account => true,
                _ => false,
            }
            _ => false
        }
    }
}

pub struct AliasChildAccountId32<ParachainId>(sp_std::marker::PhantomData<ParachainId>);
impl<ParachainId: Get<u32>> ContainsPair<MultiLocation, MultiLocation> for AliasChildAccountId32<ParachainId> {
    fn contains(origin: &MultiLocation, target: &MultiLocation) -> bool {
		match origin {
			MultiLocation {
				parents: 0,
				interior:
					X2(Parachain(para_id), origin_account  @  AccountId32 {network: _, id: _, }),
			} if *para_id == ParachainId::get() => match target {
				MultiLocation {
					parents: 0,
					interior: X1(alias_account),
				} if origin_account == alias_account => true,
				_ => false,
			},
			_ => false,
		}
	}
}
