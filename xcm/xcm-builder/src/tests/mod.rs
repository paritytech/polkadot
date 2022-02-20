// Copyright 2020 Parity Technologies query_id: (), max_response_weight: ()  query_id: (), max_response_weight: ()  (UK) Ltd.
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

use core::convert::TryInto;
use super::{test_utils::*, *};
use frame_support::{assert_err, traits::ConstU32, weights::constants::WEIGHT_PER_SECOND};
use xcm_executor::{traits::*, Config, XcmExecutor};

mod mock;
use mock::*;

mod assets;
mod barriers;
mod basic;
mod expect_pallet;
mod origins;
mod querying;
mod transacting;
mod version_subscriptions;
mod weight;
mod bridging;
