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

use crate::{
	params, prelude::*, rpc_helpers::*, signer::Signer, DryRunConfig, Error, SharedConfig, WsClient,
};
use codec::Encode;

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot<T: EPM::Config>(ext: &mut Ext) -> Result<(), Error> {
	ext.execute_with(|| {
		if <EPM::Snapshot<T>>::exists() {
			log::info!(target: LOG_TARGET, "snapshot already exists.");
			Ok(())
		} else {
			log::info!(target: LOG_TARGET, "creating a fake snapshot now.");
			<EPM::Pallet<T>>::create_snapshot().map(|_| ()).map_err(Into::into)
		}
	})
}

/// Helper method to print the encoded size of the snapshot.
fn measure_snapshot_size<T: EPM::Config>(ext: &mut Ext) {
	ext.execute_with(|| {
		log::info!(target: LOG_TARGET, "Metadata: {:?}", <EPM::Pallet<T>>::snapshot_metadata());
		log::info!(
			target: LOG_TARGET,
			"Encoded Length: {:?}",
			<EPM::Pallet<T>>::snapshot()
				.expect("snapshot must exist before calling `measure_snapshot_size`")
				.encode()
				.len()
		);
	})
}

/// Find the stake threshold in order to have at most `count` voters.
#[allow(unused)]
fn find_threshold<T: EPM::Config>(ext: &mut Ext, count: usize) {
	ext.execute_with(|| {
		let mut voters = <EPM::Pallet<T>>::snapshot()
			.expect("snapshot must exist before calling `measure_snapshot_size`")
			.voters;
		voters.sort_by_key(|(_voter, weight, _targets)| std::cmp::Reverse(*weight));
		match voters.get(count) {
			Some(threshold_voter) => println!("smallest allowed voter is {:?}", threshold_voter),
			None => {
				println!("requested truncation to {} voters but had only {}", count, voters.len());
				println!("smallest current voter: {:?}", voters.last());
			}
		}
	})
}

macro_rules! dry_run_cmd_for { ($runtime:ident) => { paste::paste! {
	/// Execute the dry-run command.
	pub(crate) async fn [<dry_run_cmd_ $runtime>](
		client: &WsClient,
		shared: SharedConfig,
		config: DryRunConfig,
		signer: Signer,
	) -> Result<(), Error> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let mut ext = crate::create_election_ext::<Runtime, Block>(shared.uri.clone(), config.at, true).await?;
		force_create_snapshot::<Runtime>(&mut ext)?;
		measure_snapshot_size::<Runtime>(&mut ext);
		let (raw_solution, witness) = crate::mine_unchecked::<Runtime>(&mut ext, config.iterations, false)?;
		log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

		let nonce = crate::get_account_info::<Runtime>(client, &signer.account, config.at)
			.await?
			.map(|i| i.nonce)
			.expect("signer account is checked to exist upon startup; it can only die if it \
				transfers funds out of it, or get slashed. If it does not exist at this point, \
				it is likely due to a bug, or the signer got slashed. Terminating."
			);
		let tip = 0 as Balance;
		let era = sp_runtime::generic::Era::Immortal;
		let extrinsic = ext.execute_with(|| create_uxt(raw_solution, witness, signer.clone(), nonce, tip, era));

		let bytes = sp_core::Bytes(extrinsic.encode().to_vec());
		let outcome = rpc_decode::<sp_runtime::ApplyExtrinsicResult>(client, "system_dryRun", params!{ bytes }).await?;
		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);
		Ok(())
	}
}}}

dry_run_cmd_for!(polkadot);
dry_run_cmd_for!(kusama);
dry_run_cmd_for!(westend);
