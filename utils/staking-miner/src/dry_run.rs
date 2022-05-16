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

//! The dry-run command.

use pallet_election_provider_multi_phase::RawSolution;

use crate::{chain, error::Error, prelude::*, DryRunConfig};
use codec::{Decode, Encode};
use jsonrpsee::rpc_params;
use subxt::{rpc::ClientT, sp_core::Bytes};

macro_rules! dry_run_cmd_for {
	($runtime:tt) => {
		paste::paste! {

			pub(crate) async fn [<run_$runtime>](api: chain::$runtime::RuntimeApi, config: DryRunConfig, signer: Signer) -> Result<(), Error>

		{
			let (solution, score, _size) =
				crate::helpers::[<mine_solution_$runtime>](&api, config.at, config.solver).await?;

			let round = api.storage().election_provider_multi_phase().round(config.at).await?;
			let raw_solution = RawSolution { solution, score, round };


			log::info!(
				target: LOG_TARGET,
				"solution score {:?} / length {:?}",
				score,
				raw_solution.encode().len(),
			);

			let uxt = api.tx().election_provider_multi_phase().submit(raw_solution)?;
			let sxt = uxt.create_signed(&signer, subxt::PolkadotExtrinsicParamsBuilder::default()).await?;


			let encoded_xt = Bytes(sxt.encode());

			let bytes: Bytes = api
				.client
				.rpc()
				.client
				.request("system_dryRun", rpc_params![encoded_xt])
				.await?;

			let outcome: sp_runtime::ApplyExtrinsicResult = Decode::decode(&mut &*bytes.0)?;

			log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);

			match outcome {
				Ok(Ok(r)) => Ok(r),
				err => panic!("{:?}", err),
			}
		}

	}
	};
}

dry_run_cmd_for!(polkadot);
dry_run_cmd_for!(kusama);
dry_run_cmd_for!(westend);
