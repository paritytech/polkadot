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

use crate::{opts::DryRunConfig, prelude::*, rpc::*, signer::Signer, Error, SharedRpcClient};
use codec::Encode;
use frame_support::traits::Currency;
use sp_core::Bytes;
use sp_npos_elections::ElectionScore;

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot<T: EPM::Config>(ext: &mut Ext) -> Result<(), Error<T>> {
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
async fn print_info<T: EPM::Config>(
	rpc: &SharedRpcClient,
	ext: &mut Ext,
	raw_solution: &EPM::RawSolution<EPM::SolutionOf<T::MinerConfig>>,
	extrinsic: &Bytes,
) where
	<T as EPM::Config>::Currency: Currency<T::AccountId, Balance = Balance>,
{
	ext.execute_with(|| {
		log::info!(
			target: LOG_TARGET,
			"Snapshot Metadata: {:?}",
			<EPM::Pallet<T>>::snapshot_metadata()
		);
		log::info!(
			target: LOG_TARGET,
			"Snapshot Encoded Length: {:?}",
			<EPM::Pallet<T>>::snapshot()
				.expect("snapshot must exist before calling `measure_snapshot_size`")
				.encode()
				.len()
		);

		let snapshot_size =
			<EPM::Pallet<T>>::snapshot_metadata().expect("snapshot must exist by now; qed.");
		let deposit = EPM::Pallet::<T>::deposit_for(raw_solution, snapshot_size);

		let score = {
			let ElectionScore { minimal_stake, sum_stake, sum_stake_squared } = raw_solution.score;
			[Token::from(minimal_stake), Token::from(sum_stake), Token::from(sum_stake_squared)]
		};

		log::info!(
			target: LOG_TARGET,
			"solution score {:?} / deposit {:?} / length {:?}",
			score,
			Token::from(deposit),
			raw_solution.encode().len(),
		);
	});

	let info = rpc.payment_query_info(&extrinsic, None).await;

	log::info!(
		target: LOG_TARGET,
		"payment_queryInfo: (fee = {}) {:?}",
		info.as_ref()
			.map(|d| Token::from(d.partial_fee))
			.unwrap_or_else(|_| Token::from(0)),
		info,
	);
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
			},
		}
	})
}

macro_rules! dry_run_cmd_for { ($runtime:ident) => { paste::paste! {
	/// Execute the dry-run command.
	pub(crate) async fn [<dry_run_cmd_ $runtime>](
		rpc: SharedRpcClient,
		config: DryRunConfig,
		signer: Signer,
	) -> Result<(), Error<$crate::[<$runtime _runtime_exports>]::Runtime>> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let pallets = if config.force_snapshot {
			vec!["Staking".to_string(), "BagsList".to_string()]
		} else {
			Default::default()
		};
		let mut ext = crate::create_election_ext::<Runtime, Block>(rpc.clone(), config.at, pallets).await?;
		if config.force_snapshot {
			force_create_snapshot::<Runtime>(&mut ext)?;
		};

		log::debug!(target: LOG_TARGET, "solving with {:?}", config.solver);
		let raw_solution = crate::mine_with::<Runtime>(&config.solver, &mut ext, false)?;

		let nonce = crate::get_account_info::<Runtime>(&rpc, &signer.account, config.at)
			.await?
			.map(|i| i.nonce)
			.expect("signer account is checked to exist upon startup; it can only die if it \
			transfers funds out of it, or get slashed. If it does not exist at this point, \
			it is likely due to a bug, or the signer got slashed. Terminating."
		);
		let tip = 0 as Balance;
		let era = sp_runtime::generic::Era::Immortal;
		let extrinsic = ext.execute_with(|| create_uxt(raw_solution.clone(), signer.clone(), nonce, tip, era));

		let bytes = sp_core::Bytes(extrinsic.encode().to_vec());
		print_info::<Runtime>(&rpc, &mut ext, &raw_solution, &bytes).await;

		let feasibility_result = ext.execute_with(|| {
			EPM::Pallet::<Runtime>::feasibility_check(raw_solution.clone(), EPM::ElectionCompute::Signed)
		});
		log::info!(target: LOG_TARGET, "feasibility result is {:?}", feasibility_result.map(|_| ()));

		let dispatch_result = ext.execute_with(|| {
			// manually tweak the phase.
			EPM::CurrentPhase::<Runtime>::put(EPM::Phase::Signed);
			EPM::Pallet::<Runtime>::submit(frame_system::RawOrigin::Signed(signer.account).into(), Box::new(raw_solution))
		});
		log::info!(target: LOG_TARGET, "dispatch result is {:?}", dispatch_result);

		let dry_run_fut = rpc.dry_run(&bytes, None);
		let outcome: sp_runtime::ApplyExtrinsicResult = await_request_and_decode(dry_run_fut).await.map_err::<Error<Runtime>, _>(Into::into)?;
		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);
		Ok(())
	}
}}}

dry_run_cmd_for!(polkadot);
dry_run_cmd_for!(kusama);
dry_run_cmd_for!(westend);
