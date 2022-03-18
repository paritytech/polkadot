// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! The emergency-solution command.

use crate::{prelude::*, EmergencySolutionConfig, Error, SharedRpcClient};
use codec::Encode;
use std::io::Write;

macro_rules! emergency_solution_cmd_for { ($runtime:ident) => { paste::paste! {
	/// Execute the emergency-solution command.
	pub(crate) async fn [<emergency_solution_cmd_ $runtime>](
		client: SharedRpcClient,
		config: EmergencySolutionConfig,
	) -> Result<(), Error<$crate::[<$runtime _runtime_exports>]::Runtime>> {
		use $crate::[<$runtime _runtime_exports>]::*;

		let mut ext = crate::create_election_ext::<Runtime, Block>(client, config.at, vec![]).await?;
		let raw_solution = crate::mine_with::<Runtime>(&config.solver, &mut ext, false)?;

		ext.execute_with(|| {
			assert!(EPM::Pallet::<Runtime>::current_phase().is_emergency());

			log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

			let mut ready_solution = EPM::Pallet::<Runtime>::feasibility_check(raw_solution, EPM::ElectionCompute::Signed)?;

			// maybe truncate.
			if let Some(take) = config.take {
				log::info!(target: LOG_TARGET, "truncating {} winners to {}", ready_solution.supports.len(), take);
				ready_solution.supports.sort_unstable_by_key(|(_, s)| s.total);
				ready_solution.supports.truncate(take);
			}

			// write to file and stdout.
			let encoded_support = ready_solution.supports.encode();
			let mut supports_file = std::fs::File::create("solution.supports.bin")?;
			supports_file.write_all(&encoded_support)?;

			log::info!(target: LOG_TARGET, "ReadySolution: size {:?} / score = {:?}", ready_solution.encoded_size(), ready_solution.score);
			log::trace!(target: LOG_TARGET, "Supports: {}", sp_core::hexdisplay::HexDisplay::from(&encoded_support));

			Ok(())
		})
	}
}}}

emergency_solution_cmd_for!(polkadot);
emergency_solution_cmd_for!(kusama);
emergency_solution_cmd_for!(westend);
