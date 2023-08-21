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

use super::{test_utils::*, *};
use core::convert::TryInto;
use frame_support::{
	assert_err,
	traits::{ConstU32, ContainsPair, ProcessMessageError},
	weights::constants::{WEIGHT_PROOF_SIZE_PER_MB, WEIGHT_REF_TIME_PER_SECOND},
};
use xcm_executor::{traits::prelude::*, Config, XcmExecutor};

mod mock;
use mock::*;

mod aliases;
mod assets;
mod barriers;
mod basic;
mod bridging;
mod expecting;
mod locking;
mod origins;
mod pay;
mod querying;
mod transacting;
mod version_subscriptions;
mod weight;
