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

use crate::{prelude::*, DryRunConfig};

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot(client: &SubxtClient) -> Result<(), Error> {
	todo!();
}

/// Find the stake threshold in order to have at most `count` voters.
#[allow(unused)]
fn find_threshold(count: usize) {
	todo!();
}

async fn run_cmd(client: SubxtClient, config: DryRunConfig, signer: Signer) -> Result<(), Error> {
	let api: RuntimeApi = client.to_runtime_api();

	todo!();

	/*let dry_run_fut = rpc.dry_run(&bytes, None);
	let outcome: sp_runtime::ApplyExtrinsicResult = await_request_and_decode(dry_run_fut)
		.await
		.map_err::<Error<Runtime>, _>(Into::into)?;
	log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);*/
	Ok(())
}
