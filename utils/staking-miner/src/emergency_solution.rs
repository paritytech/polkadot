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

use crate::{chain, prelude::*, EmergencySolutionConfig};

macro_rules! emergency_cmd_for {
	($runtime:tt) => {
		paste::paste! {

					pub(crate) async fn [<run_$runtime>](
						api: chain::$runtime::RuntimeApi,
						config: EmergencySolutionConfig,
						_signer: Signer,
					) -> Result<(), Error> {

					use pallet_election_provider_multi_phase::Phase;

					// Ensure that the current phase is emergency.
					match api.storage().election_provider_multi_phase().current_phase(config.at).await {
						Ok(Phase::Emergency) => (),
						Ok(_phase) => return Err(Error::IncorrectPhase),
						Err(e) => return Err(e.into()),
					};


					todo!("how to get ReadySolution here?!");

					/*let mut ready_solutions = api
						.storage()
						.election_provider_multi_phase()
						.queued_solution(Some(events.block_hash()))
						.await?
						.ok_or(Error::Other("queued solutions were empty".into()))?;

					// maybe truncate.
					if let Some(take) = config.take {
						log::info!(
							target: LOG_TARGET,
							"truncating {} winners to {}",
							ready_solutions.supports.len(),
							take
						);
						ready_solutions.supports.sort_unstable_by_key(|(_, s)| s.total);
						ready_solutions.supports.truncate(take);
					}

					// write to file and stdout.
					let encoded_support = ready_solutions.supports.encode();
					let mut supports_file = std::fs::File::create("solution.supports.bin")?;
					supports_file.write_all(&encoded_support)?;

					Ok(())*/
				}
		}
	};
}

emergency_cmd_for!(polkadot);
emergency_cmd_for!(kusama);
emergency_cmd_for!(westend);
