// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! We want to use a different validator configuration for benchmarking than what's used in Kovan
//! or in our Rialto test network. However, we can't configure a new validator set on the fly which
//! means we need to wire the runtime together like this

use pallet_bridge_eth_poa::{ValidatorsConfiguration, ValidatorsSource};
use sp_std::vec;

pub use crate::kovan::{
	genesis_header, genesis_validators, BridgeAuraConfiguration, FinalityVotesCachingInterval, PruningStrategy,
};

frame_support::parameter_types! {
	pub BridgeValidatorsConfiguration: pallet_bridge_eth_poa::ValidatorsConfiguration = bench_validator_config();
}

fn bench_validator_config() -> ValidatorsConfiguration {
	ValidatorsConfiguration::Multi(vec![
		(0, ValidatorsSource::List(vec![[1; 20].into()])),
		(1, ValidatorsSource::Contract([3; 20].into(), vec![[1; 20].into()])),
	])
}
