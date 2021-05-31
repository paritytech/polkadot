use crate::{prelude::*, Signer, SharedConfig, WsClient, Error, rpc, rpc_decode};
use codec::Encode;

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot<T: EPM::Config>(ext: &mut Ext) {
	ext.execute_with(|| {
		if <EPM::Snapshot<T>>::exists() {
			log::info!(target: LOG_TARGET, "snapshot already exists.");
		} else {
			log::info!(target: LOG_TARGET, "creating a fake snapshot now.");
		}
		<EPMPalletOf<T>>::create_snapshot().unwrap();
	});
}

macro_rules! dry_run_cmd_for { ($runtime:tt) => { paste::paste! {
	pub(crate) async fn [<dry_run_cmd_ $runtime>](
		client: WsClient,
		shared: SharedConfig,
		signer: Signer,
	) -> Result<(), Error> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let hash = rpc!(client<chain_getFinalizedHead, <Block as BlockT>::Hash>,).unwrap();
		let mut ext = crate::create_election_ext::<Runtime, Block>(shared.uri.clone(), hash, true).await;
		force_create_snapshot::<Runtime>(&mut ext);
		let (raw_solution, witness) = crate::mine_unchecked::<Runtime>(&mut ext);
		log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);
		let extrinsic = ext.execute_with(|| create_uxt(raw_solution, witness, signer.clone()));
		let bytes = sp_core::Bytes(extrinsic.encode().to_vec());
		let outcome = rpc_decode!(client<system_dryRun, sp_core::Bytes, sp_runtime::ApplyExtrinsicResult>, bytes);
		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);
		Ok(())
	}
}}}

dry_run_cmd_for!(polkadot);
dry_run_cmd_for!(kusama);
dry_run_cmd_for!(westend);
