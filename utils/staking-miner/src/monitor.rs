use crate::{
	prelude::*,
	runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot,
	BalanceIterations, Balancing, MonitorConfig, Solver, SubmissionStrategy,
};
use pallet_election_provider_multi_phase::RawSolution;
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{sp_core::storage::StorageKey, TransactionStatus};
use tokio::sync::mpsc;
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};

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
	at: Header,
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
		log::debug!("not signed phase; skipping");
		return
	}

	if ensure_no_previous_solution(&api, hash, signer.account_id()).await.is_err() {
		log::debug!("solution already exist; skipping");
		return
	}

	log::debug!("prep to create raw solution");
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


		let voters: Vec<_> = voters
			.into_iter()
			.map(|(a, b, mut c)| {
				let mut bounded_vec: frame_support::BoundedVec<_, MinerMaxVotesPerVotes> =
					frame_support::BoundedVec::default();

				bounded_vec.try_append(&mut c.0).unwrap();

				(a, b, bounded_vec)
			})
			.collect();

		match config.solver {
			Solver::SeqPhragmen { iterations } => {
				//BalanceIterations::set(*iterations);
				PolkadotMiner::mine_solution_with_snapshot::<
					SequentialPhragmen<AccountId, Perbill, Balancing>,
				>(voters, targets, desired_targets)
			},
			Solver::PhragMMS { iterations } => {
				//BalanceIterations::set(*iterations);
				PolkadotMiner::mine_solution_with_snapshot::<PhragMMS<AccountId, Perbill, Balancing>>(
					voters,
					targets,
					desired_targets,
				)
			},
		}
	};

	let (solution, score, solution_or_snapshot) = raw_solution.unwrap();

	let round = api.storage().election_provider_multi_phase().round(Some(hash)).await.unwrap();

	log::info!(target: LOG_TARGET, "mined solution with {:?}", score);

	let call = SubmitCall::new(RawSolution { solution, score, round });

	let xt = subxt::SubmittableExtrinsic::<_, ExtrinsicParams, _, ModuleErrMissing, NoEvents>::new(
		&api.client,
		call,
	);

	let mut status = xt
		.sign_and_submit_then_watch(
			&*signer,
			subxt::SubstrateExtrinsicParamsBuilder::<subxt::DefaultConfig>::default(),
		)
		.await
		.unwrap();

	while let Some(Ok(s)) = status.next_item().await {
		match s {
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
