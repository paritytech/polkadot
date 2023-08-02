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

//! Helper functions for tests, also used in runtime-benchmarks.

#![cfg(test)]

use super::*;

use crate::{
	mock::MockGenesisConfig,
	paras::{ParaGenesisArgs, ParaKind},
};

use primitives::{Balance, HeadData, ValidationCode};

pub fn default_genesis_config() -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: crate::configuration::HostConfiguration { ..Default::default() },
		},
		..Default::default()
	}
}

#[derive(Debug)]
pub struct GenesisConfigBuilder {
	pub on_demand_cores: u32,
	pub on_demand_base_fee: Balance,
	pub on_demand_fee_variability: Perbill,
	pub on_demand_max_queue_size: u32,
	pub on_demand_target_queue_utilization: Perbill,
	pub onboarded_on_demand_chains: Vec<ParaId>,
}

impl Default for GenesisConfigBuilder {
	fn default() -> Self {
		Self {
			on_demand_cores: 10,
			on_demand_base_fee: 10_000,
			on_demand_fee_variability: Perbill::from_percent(1),
			on_demand_max_queue_size: 100,
			on_demand_target_queue_utilization: Perbill::from_percent(25),
			onboarded_on_demand_chains: vec![],
		}
	}
}

impl GenesisConfigBuilder {
	pub(super) fn build(self) -> MockGenesisConfig {
		let mut genesis = default_genesis_config();
		let config = &mut genesis.configuration.config;
		config.on_demand_cores = self.on_demand_cores;
		config.on_demand_base_fee = self.on_demand_base_fee;
		config.on_demand_fee_variability = self.on_demand_fee_variability;
		config.on_demand_queue_max_size = self.on_demand_max_queue_size;
		config.on_demand_target_queue_utilization = self.on_demand_target_queue_utilization;

		let paras = &mut genesis.paras.paras;
		for para_id in self.onboarded_on_demand_chains {
			paras.push((
				para_id,
				ParaGenesisArgs {
					genesis_head: HeadData::from(vec![0u8]),
					validation_code: ValidationCode::from(vec![0u8]),
					para_kind: ParaKind::Parathread,
				},
			))
		}

		genesis
	}
}
