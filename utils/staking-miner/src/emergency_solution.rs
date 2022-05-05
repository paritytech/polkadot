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

/*use codec::Encode;
use pallet_election_provider_multi_phase::RawSolution;
use std::io::Write;*/

use crate::{prelude::*, EmergencySolutionConfig};

/// Ensure that the current phase is emergency.
async fn ensure_emergency_phase(api: &RuntimeApi, hash: Option<Hash>) -> Result<(), Error> {
	use runtime::pallet_election_provider_multi_phase::Phase;

	match api.storage().election_provider_multi_phase().current_phase(hash).await {
		Ok(Phase::Emergency) => Ok(()),
		Ok(_phase) => Err(Error::IncorrectPhase),
		Err(e) => Err(e.into()),
	}
}

pub(crate) async fn run<M>(
	client: SubxtClient,
	config: EmergencySolutionConfig,
	signer: Signer,
) -> Result<(), Error>
where
	M: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = crate::chains::MinerMaxVotesPerVoter>
		+ 'static,
	<M as MinerConfig>::Solution: Send + Sync,
{
	let api: RuntimeApi = client.to_runtime_api();
	ensure_emergency_phase(&api, config.at).await?;

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
	supports_file.write_all(&encoded_support)?;*/

	Ok(())
}
