use crate::{
	prelude::*,
	runtime::runtime_types::{
		pallet_election_provider_multi_phase::{self, RoundSnapshot},
		sp_core::crypto::AccountId32,
	},
	BalanceIterations, Balancing, MonitorConfig, Solver, SubmissionStrategy,
};
use codec::Encode;
use jsonrpsee::core::Error as RpcError;
use sc_transaction_pool_api::TransactionStatus;
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::sp_core::storage::StorageKey;
use tokio::sync::mpsc;

/// Ensure that now is the signed phase.
async fn ensure_signed_phase(api: &RuntimeApi, hash: Hash) -> Result<(), anyhow::Error> {
	use runtime::pallet_election_provider_multi_phase::Phase;

	match api.storage().election_provider_multi_phase().current_phase(Some(hash)).await {
		Ok(Phase::Signed) => Ok(()),
		Ok(p) => Err(anyhow::anyhow!("Incorrect phase")),
		Err(e) => Err(e.into()),
	}
}

/// Ensure that our current `us` have not submitted anything previously.
async fn ensure_no_previous_solution(
	api: &RuntimeApi,
	at: Hash,
	us: &subxt::sp_runtime::AccountId32,
) -> Result<(), anyhow::Error> {
	let indices = api
		.storage()
		.election_provider_multi_phase()
		.signed_submission_indices(Some(at))
		.await?;

	for (_score, idx) in indices.0 {
		let submission = api
			.storage()
			.election_provider_multi_phase()
			.signed_submissions_map(&idx, Some(at))
			.await?;

		if let Some(submission) = submission {
			if &submission.who == us {
				return Err(anyhow::anyhow!("Already submitted"))
			}
		}
	}

	Ok(())
}

/// Reads all current solutions and checks the scores according to the `SubmissionStrategy`.
async fn ensure_no_better_solution(
	api: &RuntimeApi,
	at: Hash,
	score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
) -> Result<(), anyhow::Error> {
	let epsilon = match strategy {
		// don't care about current scores.
		SubmissionStrategy::Always => return Ok(()),
		SubmissionStrategy::IfLeading => Perbill::zero(),
		SubmissionStrategy::ClaimBetterThan(epsilon) => epsilon,
	};

	let indices = api
		.storage()
		.election_provider_multi_phase()
		.signed_submission_indices(Some(at))
		.await?;

	for (s, _) in indices.0 {
		let other_score = sp_npos_elections::ElectionScore {
			minimal_stake: s.minimal_stake,
			sum_stake: s.sum_stake,
			sum_stake_squared: s.sum_stake_squared,
		};

		if !score.strict_threshold_better(other_score, epsilon) {
			return Err(anyhow::anyhow!("better score already exist"))
		}
	}

	Ok(())
}

pub(crate) async fn run_cmd(
	client: SubxtClient,
	config: MonitorConfig,
	signer: Arc<Signer>,
) -> Result<(), ()> {
	/*let heads_subscription = || {
		if config.listen == "head" {
			api.client.rpc().subscribe_blocks()
		} else {
			api.client.rpc().subscribe_finalized_blocks()
		}
	};*/

	let mut subscription = client.rpc().subscribe_finalized_blocks().await.map_err(|_| ())?;

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

	loop {
		let at = tokio::select! {
			maybe_rp = subscription.next() => {
				match maybe_rp {
					Some(Ok(r)) => r,
					Some(Err(e)) => {
						log::error!(target: LOG_TARGET, "subscription failed to decode Header {:?}, this is bug please file an issue", e);
						return Err(());
					}
					// The subscription was dropped, should only happen if:
					//	- the connection was closed.
					//	- the subscription could not keep up with the server.
					None => {
						log::warn!(target: LOG_TARGET, "subscription to `subscribeNewHeads/subscribeFinalizedHeads` terminated. Retrying..");
						subscription = client.rpc().subscribe_finalized_blocks().await.map_err(|_| ())?;
						continue
					}
				}
			},
			maybe_err = rx.recv() => {
				match maybe_err {
					Some(err) => return Err(err),
					None => unreachable!("at least one sender kept in the main loop should always return Some; qed"),
				}
			}
		};

		// Spawn task and non-recoverable errors are sent back to the main task
		// such as if the connection has been closed.
		tokio::spawn(send_and_watch_extrinsic(
			tx.clone(),
			at,
			client.clone(),
			signer.clone(),
			config.clone(),
		));
	}
}

/// Construct extrinsic at given block and watch it.
async fn send_and_watch_extrinsic(
	tx: tokio::sync::mpsc::UnboundedSender<()>,
	at: subxt::sp_runtime::generic::Header<u32, subxt::sp_runtime::traits::BlakeTwo256>,
	client: SubxtClient,
	signer: Arc<Signer>,
	config: MonitorConfig,
) {
	async fn flatten<T>(handle: tokio::task::JoinHandle<Result<T, ()>>) -> Result<T, ()> {
		match handle.await {
			Ok(Ok(result)) => Ok(result),
			Ok(Err(err)) => Err(err),
			Err(err) => panic!("tokio spawn task failed; kill task: {:?}", err),
		}
	}

	let hash = at.hash();
	log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

	let api: RuntimeApi = client.to_runtime_api();

	if ensure_signed_phase(&api, hash).await.is_err() {
		return
	}

	if ensure_no_previous_solution(&api, hash, signer.account_id()).await.is_err() {
		return
	}

	let raw_solution = {
		let RoundSnapshot { voters, targets } = api
			.storage()
			.election_provider_multi_phase()
			.snapshot(Some(hash))
			.await
			.unwrap()
			.unwrap_or_else(|| RoundSnapshot { voters: Vec::new(), targets: Vec::new() });

		let desired_targets = api
			.storage()
			.election_provider_multi_phase()
			.desired_targets(Some(hash))
			.await
			.unwrap()
			.unwrap_or_default();

		use frame_election_provider_support::{PhragMMS, SequentialPhragmen};


		match config.solver {
			Solver::SeqPhragmen { iterations } => {
				//BalanceIterations::set(*iterations);

				type S = SequentialPhragmen<AccountId32, Perbill, Balancing>;
				Miner::mine_solution_with_snapshot::<S>(voters, targets, desired_targets);
			},
			Solver::PhragMMS { iterations } => {
				//BalanceIterations::set(*iterations);
				//PhragMMS::<AccountId32, Perbill, Balancing>::solve();
				todo!();
			},
		}
	};

	/*

	// mine a solution, and run feasibility check on it as well.
	let raw_solution = match crate::mine_with::<Runtime>(&config.solver, &mut ext, true) {
		Ok(r) => r,
		Err(err) => {
			let _ = tx.send(err.into());
			return;
		},
	};

	let score = raw_solution.score;
	log::info!(target: LOG_TARGET, "mined solution with {:?}", score);

	let nonce = match crate::get_account_info::<Runtime>(&rpc, &signer.account, Some(hash)).await {
		Ok(maybe_account) => {
			let acc = maybe_account.expect(crate::signer::SIGNER_ACCOUNT_WILL_EXIST);
			acc.nonce
		},
		Err(err) => {
			let _ = tx.send(err);
			return;
		},
	};

	let tip = 0 as Balance;
	let period = <Runtime as frame_system::Config>::BlockHashCount::get() / 2;
	let current_block = at.number.saturating_sub(1);
	let era = sp_runtime::generic::Era::mortal(period.into(), current_block.into());

	log::trace!(
		target: LOG_TARGET,
		"transaction mortality: {:?} -> {:?}",
		era.birth(current_block.into()),
		era.death(current_block.into()),
	);

	let extrinsic = ext.execute_with(|| create_uxt(raw_solution, signer.clone(), nonce, tip, era));
	let bytes = sp_core::Bytes(extrinsic.encode());

	let rpc1 = rpc.clone();
	let rpc2 = rpc.clone();

	let ensure_no_better_fut = tokio::spawn(async move {
		ensure_no_better_solution::<Runtime, Block>(&rpc1, hash, score, config.submission_strategy)
			.await
	});

	let ensure_signed_phase_fut =
		tokio::spawn(async move { ensure_signed_phase::<Runtime, Block>(&rpc2, hash).await });

	// Run the calls in parallel and return once all has completed or any failed.
	if tokio::try_join!(flatten(ensure_no_better_fut), flatten(ensure_signed_phase_fut),).is_err() {
		return;
	}

	let mut tx_subscription = match rpc.watch_extrinsic(&bytes).await {
		Ok(sub) => sub,
		Err(RpcError::RestartNeeded(e)) => {
			let _ = tx.send(RpcError::RestartNeeded(e).into());
			return;
		},
		Err(why) => {
			// This usually happens when we've been busy with mining for a few blocks, and
			// now we're receiving the subscriptions of blocks in which we were busy. In
			// these blocks, we still don't have a solution, so we re-compute a new solution
			// and submit it with an outdated `Nonce`, which yields most often `Stale`
			// error. NOTE: to improve this overall, and to be able to introduce an array of
			// other fancy features, we should make this multi-threaded and do the
			// computation outside of this callback.
			log::warn!(
				target: LOG_TARGET,
				"failing to submit a transaction {:?}. ignore block: {}",
				why,
				at.number
			);
			return;
		},
	};

	while let Some(rp) = tx_subscription.next().await {
		let status_update = match rp {
			Ok(r) => r,
			// Custom `jsonrpsee` message sent by the server if the subscription was closed on the server side.
			Err(RpcError::SubscriptionClosed(reason)) => {
				log::warn!(
					target: LOG_TARGET,
					"tx subscription closed by the server: {:?}; skip block: {}",
					reason,
					at.number
				);
				return;
			},
			Err(e) => {
				log::error!(target: LOG_TARGET, "subscription failed to decode TransactionStatus {:?}, this is a bug please file an issue", e);
				let _ = tx.send(e.into());
				return;
			},
		};

		log::trace!(target: LOG_TARGET, "status update {:?}", status_update);
		match status_update {
			TransactionStatus::Ready
			| TransactionStatus::Broadcast(_)
			| TransactionStatus::Future => continue,
			TransactionStatus::InBlock(hash) => {
				log::info!(target: LOG_TARGET, "included at {:?}", hash);
				let key = StorageKey(
					frame_support::storage::storage_prefix(b"System", b"Events").to_vec(),
				);

				let events = match rpc
					.get_storage_and_decode::<Vec<frame_system::EventRecord<Event, <Block as BlockT>::Hash>>>(
						&key,
						Some(hash),
					)
					.await
				{
					Ok(rp) => rp.unwrap_or_default(),
					Err(RpcHelperError::JsonRpsee(RpcError::RestartNeeded(e))) => {
						let _ = tx.send(RpcError::RestartNeeded(e).into());
						return;
					},
					// Decoding or other RPC error => just terminate the task.
					Err(e) => {
						log::warn!(
							target: LOG_TARGET,
							"get_storage [key: {:?}, hash: {:?}] failed: {:?}; skip block: {}",
							key,
							hash,
							e,
							at.number
						);
						return;
					},
				};

				log::info!(target: LOG_TARGET, "events at inclusion {:?}", events);
			},
			TransactionStatus::Retracted(hash) => {
				log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
			},
			TransactionStatus::Finalized(hash) => {
				log::info!(target: LOG_TARGET, "Finalized at {:?}", hash);
				break;
			},
			_ => {
				log::warn!(
					target: LOG_TARGET,
					"Stopping listen due to other status {:?}",
					status_update
				);
				break;
			},
		};
	}*/
}
