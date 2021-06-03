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

use crate::{prelude::*, Signer, SharedConfig, DryRunConfig, WsClient, Error, rpc_helpers::*, params};
use codec::Encode;

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot<T: EPM::Config>(ext: &mut Ext) -> Result<(), Error> {
	ext.execute_with(|| {
		if <EPM::Snapshot<T>>::exists() {
			log::info!(target: LOG_TARGET, "snapshot already exists.");
		} else {
			log::info!(target: LOG_TARGET, "creating a fake snapshot now.");
		}
		let _ = <EPM::Pallet<T>>::create_snapshot()?;
		Ok(())
	})
}

macro_rules! dry_run_cmd_for { ($runtime:ident) => { paste::paste! {
	/// Execute the dry-run command.
	pub(crate) async fn [<dry_run_cmd_ $runtime>](
		client: WsClient,
		shared: SharedConfig,
		config: DryRunConfig,
		signer: Signer,
	) -> Result<(), Error> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let mut ext = crate::create_election_ext::<Runtime, Block>(shared.uri.clone(), config.at, true).await?;
		force_create_snapshot::<Runtime>(&mut ext)?;
		let (raw_solution, witness) = crate::mine_unchecked::<Runtime>(&mut ext)?;
		log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

		let nonce = crate::get_account_info::<Runtime>(&client, &signer.account, config.at)
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
		let outcome = rpc_decode::<sp_runtime::ApplyExtrinsicResult>(&client, "system_dryRun", params!{ bytes }).await?;
		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);
		Ok(())
	}
}}}

dry_run_cmd_for!(polkadot);
dry_run_cmd_for!(kusama);
dry_run_cmd_for!(westend);
