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

use crate::{prelude::*, Signer, SharedConfig, WsClient, Error, rpc_helpers::*, params};
use codec::Encode;

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot<T: EPM::Config>(ext: &mut Ext) {
	ext.execute_with(|| {
		if <EPM::Snapshot<T>>::exists() {
			log::info!(target: LOG_TARGET, "snapshot already exists.");
		} else {
			log::info!(target: LOG_TARGET, "creating a fake snapshot now.");
		}
		<EPM::Pallet<T>>::create_snapshot().unwrap();
	});
}

macro_rules! dry_run_cmd_for { ($runtime:tt) => { paste::paste! {
	/// Execute the dry-run command.
	pub(crate) async fn [<dry_run_cmd_ $runtime>](
		client: WsClient,
		shared: SharedConfig,
		signer: Signer,
	) -> Result<(), Error> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let hash = rpc::<<Block as BlockT>::Hash>(&client, "chain_getFinalizedHead", params!{}).await.expect("chain_getFinalizedHead infallible; qed.");
		let mut ext = crate::create_election_ext::<Runtime, Block>(shared.uri.clone(), hash, true).await;
		force_create_snapshot::<Runtime>(&mut ext);
		let (raw_solution, witness) = crate::mine_unchecked::<Runtime>(&mut ext);
		log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);
		let extrinsic = ext.execute_with(|| create_uxt(raw_solution, witness, signer.clone()));
		let bytes = sp_core::Bytes(extrinsic.encode().to_vec());
		let outcome = rpc_decode::<sp_runtime::ApplyExtrinsicResult>(&client, "system_dryRun", params!{ bytes }).await?;
		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);
		Ok(())
	}
}}}

dry_run_cmd_for!(polkadot);
dry_run_cmd_for!(kusama);
dry_run_cmd_for!(westend);
