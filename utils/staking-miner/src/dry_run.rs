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

use crate::{error::Error, prelude::*, DryRunConfig};
use codec::{Decode, Encode};
use jsonrpsee::rpc_params;
use subxt::{rpc::ClientT, sp_core::Bytes};

pub(crate) async fn run<M>(
	client: SubxtClient,
	config: DryRunConfig,
	signer: Signer,
) -> Result<(), Error>
where
	M: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = crate::chains::MinerMaxVotesPerVoter>
		+ 'static,
	<M as MinerConfig>::Solution: Send + Sync,
{
	let api: RuntimeApi = client.to_runtime_api();

	let (solution, score, size) =
		crate::helpers::mine_solution::<M>(&api, config.at, config.solver).await?;

	let round = api.storage().election_provider_multi_phase().round(config.at).await?;
	let call = SubmitCall::new(RawSolution { solution, score, round });

	let xt = subxt::SubmittableExtrinsic::<_, ExtrinsicParams, _, ModuleErrMissing, NoEvents>::new(
		&api.client,
		call,
	)
	.create_signed(&signer, subxt::SubstrateExtrinsicParamsBuilder::default())
	.await?;

	let encoded_xt = Bytes(xt.encode());

	let bytes: Bytes = api
		.client
		.rpc()
		.client
		.request("system_dryRun", rpc_params![encoded_xt])
		.await?;
	let outcome: sp_runtime::ApplyExtrinsicResult = Decode::decode(&mut &*bytes.0).unwrap();

	log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);

	Ok(())
}
