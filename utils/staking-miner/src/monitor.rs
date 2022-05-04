use crate::{
	prelude::*, runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot,
	BalanceIterations, Balancing, MonitorConfig, Solver, SubmissionStrategy,
};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use pallet_election_provider_multi_phase::{Miner, RawSolution};
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{sp_core::storage::StorageKey, TransactionStatus};

/// Ensure that now is the signed phase.
async fn ensure_signed_phase(api: &RuntimeApi, hash: Hash) -> Result<(), Error> {
	use runtime::pallet_election_provider_multi_phase::Phase;

	match api.storage().election_provider_multi_phase().current_phase(Some(hash)).await {
		Ok(Phase::Signed) => Ok(()),
		Ok(p) => Err(Error::IncorrectPhase),
		Err(e) => Err(e.into()),
	}
}

/// Ensure that our current `us` have not submitted anything previously.
async fn ensure_no_previous_solution(
	api: &RuntimeApi,
	at: Hash,
	us: &subxt::sp_runtime::AccountId32,
) -> Result<(), Error> {
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
				return Err(Error::AlreadySubmitted)
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
) -> Result<(), Error> {
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
			return Err(Error::BetterScoreExist)
		}
	}

	Ok(())
}

pub(crate) async fn run_cmd(
	client: SubxtClient,
	chain: Chain,
	config: MonitorConfig,
	signer: Arc<Signer>,
) -> Result<(), Error> {
	let mut subscription = if config.listen == "head" {
		client.rpc().subscribe_blocks().await
	} else {
		client.rpc().subscribe_finalized_blocks().await
	}?;

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();

	loop {
		let at = tokio::select! {
			maybe_rp = subscription.next() => {
				match maybe_rp {
					Some(Ok(r)) => r,
					Some(Err(e)) => {
						log::error!(target: LOG_TARGET, "subscription failed to decode Header {:?}, this is bug please file an issue", e);
						return Err(e.into());
					}
					// The subscription was dropped, should only happen if:
					//	- the connection was closed.
					//	- the subscription could not keep up with the server.
					None => {
						log::warn!(target: LOG_TARGET, "subscription to `subscribeNewHeads/subscribeFinalizedHeads` terminated. Retrying..");
						subscription = if config.listen == "head" {
							client.rpc().subscribe_blocks().await
						} else {
							client.rpc().subscribe_finalized_blocks().await
						}?;

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
			chain.clone(),
			client.clone(),
			signer.clone(),
			config.clone(),
		));
	}
}

/// Construct extrinsic at given block and watch it.
async fn send_and_watch_extrinsic(
	tx: tokio::sync::mpsc::UnboundedSender<Error>,
	at: Header,
	chain: Chain,
	client: SubxtClient,
	signer: Arc<Signer>,
	config: MonitorConfig,
) {
	async fn flatten<T>(handle: tokio::task::JoinHandle<Result<T, Error>>) -> Result<T, Error> {
		match handle.await {
			Ok(Ok(result)) => Ok(result),
			Ok(Err(err)) => Err(err),
			Err(err) => panic!("tokio spawn task failed; kill task: {:?}", err),
		}
	}

	let hash = at.hash();
	log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

	let api: RuntimeApi = client.to_runtime_api();

	if let Err(e) = ensure_signed_phase(&api, hash).await {
		log::debug!("ensure_signed_phase failed: {:?}; skipping", e);
		return
	}

	if let Err(e) = ensure_no_previous_solution(&api, hash, signer.account_id()).await {
		log::debug!("ensure_no_previous_solution failed: {:?}; skipping at: {}", e, at.number);
		return
	}

	log::debug!("prep to create raw solution");
	let xt = {
		match chain {
			Chain::Polkadot => {
				let (solution, score, _) =
					match crate::helpers::mine_solution_polkadot(&api, hash, config.solver).await {
						Ok(s) => s,
						Err(e) => {
							let _ = tx.send(e.into());
							return
						},
					};

				let round =
					match api.storage().election_provider_multi_phase().round(Some(hash)).await {
						Ok(r) => r,
						Err(e) => {
							let _ = tx.send(e.into());
							return
						},
					};

				log::info!(target: LOG_TARGET, "mined solution with {:?}", score);

				if let Err(e) =
					ensure_no_better_solution(&api, at.hash(), score, config.submission_strategy)
						.await
				{
					log::debug!("ensure_no_better_solution failed: {:?}", e);
					return
				}

				if let Err(e) = ensure_signed_phase(&api, hash).await {
					log::debug!("ensure_signed_phase failed: {:?}; skipping", e);
					return
				}

				let call = SubmitCall::new(RawSolution { solution, score, round });

				subxt::SubmittableExtrinsic::<
					_,
					ExtrinsicParams,
					_,
					ModuleErrMissing,
					NoEvents,
				>::new(&api.client, call)
			},
			Chain::Kusama => {
				let (solution, score, _) =
					match crate::helpers::mine_solution_kusama(&api, hash, config.solver).await {
						Ok(s) => s,
						Err(e) => {
							let _ = tx.send(e.into());
							return
						},
					};
				let round =
					match api.storage().election_provider_multi_phase().round(Some(hash)).await {
						Ok(r) => r,
						Err(e) => {
							let _ = tx.send(e.into());
							return
						},
					};

				log::info!(target: LOG_TARGET, "mined solution with {:?}", score);

				if let Err(e) =
					ensure_no_better_solution(&api, at.hash(), score, config.submission_strategy)
						.await
				{
					log::debug!("ensure_no_better_solution failed: {:?}", e);
					return
				}

				if let Err(e) = ensure_signed_phase(&api, hash).await {
					log::debug!("ensure_signed_phase failed: {:?}; skipping", e);
					return
				}

				let call = SubmitCall::new(RawSolution { solution, score, round });

				subxt::SubmittableExtrinsic::<
					_,
					ExtrinsicParams,
					_,
					ModuleErrMissing,
					NoEvents,
				>::new(&api.client, call)
			},
			Chain::Westend => {
				todo!();
			},
		}
	};

	// This might fail with outdated nonce let it just crash if that happens.
	let mut status_sub = xt
		.sign_and_submit_then_watch(
			&*signer,
			subxt::SubstrateExtrinsicParamsBuilder::<subxt::DefaultConfig>::default(),
		)
		.await
		.unwrap();

	loop {
		let status = match status_sub.next_item().await {
			Some(Ok(status)) => status,
			Some(err) => {
				log::error!(
					target: LOG_TARGET,
					"watch submit extrinsic at {:?} failed: {:?}",
					at.number(),
					err
				);
				return
			},
			None => {
				log::error!(
					target: LOG_TARGET,
					"watch submit extrinsic at {:?} closed",
					at.number(),
				);
				return
			},
		};

		match status {
			TransactionStatus::Ready |
			TransactionStatus::Broadcast(_) |
			TransactionStatus::Future => (),
			TransactionStatus::InBlock(hash) => {
				log::info!(target: LOG_TARGET, "included at {:?}", hash);
				// let mut system_events = api
				// .events()
				// .subscribe()
				// .await
				// .unwrap()
				// .filter_events::<(crate::runtime::system::events::ExtrinsicSuccess,)>();
			},
			TransactionStatus::Retracted(hash) => {
				log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
			},
			TransactionStatus::Finalized(hash) => {
				log::info!(target: LOG_TARGET, "Finalized at {:?}", hash);
				break
			},
			_ => {
				log::warn!(target: LOG_TARGET, "Stopping listen due to other status {:?}", status);
				break
			},
		}
	}
}
