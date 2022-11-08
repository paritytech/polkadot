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

//! The monitor command.

use crate::{
	prelude::*, rpc::*, signer::Signer, Error, MonitorConfig, SharedRpcClient, SubmissionStrategy,
};
use codec::Encode;
use jsonrpsee::core::Error as RpcError;
use sc_transaction_pool_api::TransactionStatus;
use sp_core::storage::StorageKey;
use sp_runtime::Perbill;
use tokio::sync::mpsc;
use EPM::{signed::SubmissionIndicesOf, SignedSubmissionOf};

/// Ensure that now is the signed phase.
async fn ensure_signed_phase<T: EPM::Config, B: BlockT<Hash = Hash>>(
	rpc: &SharedRpcClient,
	at: B::Hash,
) -> Result<(), Error<T>> {
	let key = StorageKey(EPM::CurrentPhase::<T>::hashed_key().to_vec());
	let phase = rpc
		.get_storage_and_decode::<EPM::Phase<BlockNumber>>(&key, Some(at))
		.await
		.map_err::<Error<T>, _>(Into::into)?
		.unwrap_or_default();

	if phase.is_signed() {
		Ok(())
	} else {
		Err(Error::IncorrectPhase)
	}
}

/// Ensure that our current `us` have not submitted anything previously.
async fn ensure_no_previous_solution<T, B>(
	rpc: &SharedRpcClient,
	at: Hash,
	us: &AccountId,
) -> Result<(), Error<T>>
where
	T: EPM::Config + frame_system::Config<AccountId = AccountId, Hash = Hash>,
	B: BlockT,
{
	let indices_key = StorageKey(EPM::SignedSubmissionIndices::<T>::hashed_key().to_vec());

	let indices: SubmissionIndicesOf<T> = rpc
		.get_storage_and_decode(&indices_key, Some(at))
		.await
		.map_err::<Error<T>, _>(Into::into)?
		.unwrap_or_default();

	for (_score, _bn, idx) in indices {
		let key = StorageKey(EPM::SignedSubmissionsMap::<T>::hashed_key_for(idx));

		if let Some(submission) = rpc
			.get_storage_and_decode::<SignedSubmissionOf<T>>(&key, Some(at))
			.await
			.map_err::<Error<T>, _>(Into::into)?
		{
			if &submission.who == us {
				return Err(Error::AlreadySubmitted)
			}
		}
	}

	Ok(())
}

/// `true` if `our_score` should pass the onchain `best_score` with the given strategy.
pub(crate) fn score_passes_strategy(
	our_score: sp_npos_elections::ElectionScore,
	best_score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
) -> bool {
	match strategy {
		SubmissionStrategy::Always => true,
		SubmissionStrategy::IfLeading =>
			our_score == best_score ||
				our_score.strict_threshold_better(best_score, Perbill::zero()),
		SubmissionStrategy::ClaimBetterThan(epsilon) =>
			our_score.strict_threshold_better(best_score, epsilon),
		SubmissionStrategy::ClaimNoWorseThan(epsilon) =>
			!best_score.strict_threshold_better(our_score, epsilon),
	}
}

/// Reads all current solutions and checks the scores according to the `SubmissionStrategy`.
async fn ensure_strategy_met<T: EPM::Config, B: BlockT>(
	rpc: &SharedRpcClient,
	at: Hash,
	score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
	max_submissions: u32,
) -> Result<(), Error<T>> {
	// don't care about current scores.
	if matches!(strategy, SubmissionStrategy::Always) {
		return Ok(())
	}

	let indices_key = StorageKey(EPM::SignedSubmissionIndices::<T>::hashed_key().to_vec());

	let indices: SubmissionIndicesOf<T> = rpc
		.get_storage_and_decode(&indices_key, Some(at))
		.await
		.map_err::<Error<T>, _>(Into::into)?
		.unwrap_or_default();

	if indices.len() >= max_submissions as usize {
		log::debug!(target: LOG_TARGET, "The submissions queue is full");
	}

	// default score is all zeros, any score is better than it.
	let best_score = indices.last().map(|(score, _, _)| *score).unwrap_or_default();
	log::debug!(target: LOG_TARGET, "best onchain score is {:?}", best_score);

	if score_passes_strategy(score, best_score, strategy) {
		Ok(())
	} else {
		Err(Error::StrategyNotSatisfied)
	}
}

async fn get_latest_head<T: EPM::Config>(
	rpc: &SharedRpcClient,
	mode: &str,
) -> Result<Hash, Error<T>> {
	if mode == "head" {
		match rpc.block_hash(None).await {
			Ok(Some(hash)) => Ok(hash),
			Ok(None) => Err(Error::Other("Best head not found".into())),
			Err(e) => Err(e.into()),
		}
	} else {
		rpc.finalized_head().await.map_err(Into::into)
	}
}

macro_rules! monitor_cmd_for { ($runtime:tt) => { paste::paste! {

	/// The monitor command.
	pub(crate) async fn [<monitor_cmd_ $runtime>](
		rpc: SharedRpcClient,
		config: MonitorConfig,
		signer: Signer,
	) -> Result<(), Error<$crate::[<$runtime _runtime_exports>]::Runtime>> {
		use $crate::[<$runtime _runtime_exports>]::*;
		type StakingMinerError = Error<$crate::[<$runtime _runtime_exports>]::Runtime>;

		let heads_subscription = ||
			if config.listen == "head" {
				rpc.subscribe_new_heads()
			} else {
				rpc.subscribe_finalized_heads()
			};

		let mut subscription = heads_subscription().await?;
		let (tx, mut rx) = mpsc::unbounded_channel::<StakingMinerError>();

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
							subscription = heads_subscription().await?;
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
			tokio::spawn(
				send_and_watch_extrinsic(rpc.clone(), tx.clone(), at, signer.clone(), config.clone())
			);

		}

		/// Construct extrinsic at given block and watch it.
		async fn send_and_watch_extrinsic(
			rpc: SharedRpcClient,
			tx: mpsc::UnboundedSender<StakingMinerError>,
			at: Header,
			signer: Signer,
			config: MonitorConfig,
		) {

			async fn flatten<T>(
				handle: tokio::task::JoinHandle<Result<T, StakingMinerError>>
			) -> Result<T, StakingMinerError> {
				match handle.await {
					Ok(Ok(result)) => Ok(result),
					Ok(Err(err)) => Err(err),
					Err(err) => panic!("tokio spawn task failed; kill task: {:?}", err),
				}
			}

			let hash = at.hash();
			log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

			// block on this because if this fails there is no way to recover from
			// that error i.e, upgrade/downgrade required.
			if let Err(err) = crate::check_versions::<Runtime>(&rpc, false).await {
				let _ = tx.send(err.into());
				return;
			}

			let rpc1 = rpc.clone();
			let rpc2 = rpc.clone();
			let account = signer.account.clone();

			let signed_phase_fut = tokio::spawn(async move {
				ensure_signed_phase::<Runtime, Block>(&rpc1, hash).await
			});

			tokio::time::sleep(std::time::Duration::from_secs(config.delay as u64)).await;

			let no_prev_sol_fut = tokio::spawn(async move {
				ensure_no_previous_solution::<Runtime, Block>(&rpc2, hash, &account).await
			});

			// Run the calls in parallel and return once all has completed or any failed.
			if let Err(err) = tokio::try_join!(flatten(signed_phase_fut), flatten(no_prev_sol_fut)) {
				log::debug!(target: LOG_TARGET, "Skipping block {}; {}", at.number, err);
				return;
			}

			let mut ext = match crate::create_election_ext::<Runtime, Block>(rpc.clone(), Some(hash), vec![]).await {
				Ok(ext) => ext,
				Err(err) => {
					log::debug!(target: LOG_TARGET, "Skipping block {}; {}", at.number, err);
					return;
				}
			};

			// mine a solution, and run feasibility check on it as well.
			let raw_solution = match crate::mine_with::<Runtime>(&config.solver, &mut ext, true) {
				Ok(r) => r,
				Err(err) => {
					let _ = tx.send(err.into());
					return;
				}
			};

			let score = raw_solution.score;
			log::info!(target: LOG_TARGET, "mined solution with {:?}", score);

			let nonce = match crate::get_account_info::<Runtime>(&rpc, &signer.account, Some(hash)).await {
				Ok(maybe_account) => {
					let acc = maybe_account.expect(crate::signer::SIGNER_ACCOUNT_WILL_EXIST);
					acc.nonce
				}
				Err(err) => {
					let _ = tx.send(err);
					return;
				}
			};

			let tip = 0 as Balance;
			let period = <Runtime as frame_system::Config>::BlockHashCount::get() / 2;
			let current_block = at.number.saturating_sub(1);
			let era = sp_runtime::generic::Era::mortal(period.into(), current_block.into());

			log::trace!(
				target: LOG_TARGET, "transaction mortality: {:?} -> {:?}",
				era.birth(current_block.into()),
				era.death(current_block.into()),
			);

			let extrinsic = ext.execute_with(|| create_uxt(raw_solution, signer.clone(), nonce, tip, era));
			let bytes = sp_core::Bytes(extrinsic.encode());

			let rpc1 = rpc.clone();
			let rpc2 = rpc.clone();

			let latest_head = match get_latest_head::<Runtime>(&rpc, &config.listen).await {
				Ok(hash) => hash,
				Err(e) => {
					log::debug!(target: LOG_TARGET, "Skipping to submit at block {}; {}", at.number, e);
					return;
				}
			};

			let ensure_strategy_met_fut = tokio::spawn(async move {
				ensure_strategy_met::<Runtime, Block>(
					&rpc1,
					latest_head,
					score,
					config.submission_strategy,
					SignedMaxSubmissions::get()
				).await
			});

			let ensure_signed_phase_fut = tokio::spawn(async move {
				ensure_signed_phase::<Runtime, Block>(&rpc2, latest_head).await
			});

			// Run the calls in parallel and return once all has completed or any failed.
			if let Err(err) = tokio::try_join!(
				flatten(ensure_strategy_met_fut),
				flatten(ensure_signed_phase_fut),
			) {
				log::debug!(target: LOG_TARGET, "Skipping to submit at block {}; {}", at.number, err);
				return;
			}

			let mut tx_subscription = match rpc.watch_extrinsic(&bytes).await {
				Ok(sub) => sub,
				Err(RpcError::RestartNeeded(e)) => {
					let _ = tx.send(RpcError::RestartNeeded(e).into());
					return
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
						why, at.number
					);
					return;
				},
			};

			while let Some(rp) = tx_subscription.next().await {
				let status_update = match rp {
					Ok(r) => r,
					Err(e) => {
						log::error!(target: LOG_TARGET, "subscription failed to decode TransactionStatus {:?}, this is a bug please file an issue", e);
						let _ = tx.send(e.into());
						return;
					},
				};

				log::trace!(target: LOG_TARGET, "status update {:?}", status_update);
				match status_update {
					TransactionStatus::Ready |
					TransactionStatus::Broadcast(_) |
					TransactionStatus::Future => continue,
					TransactionStatus::InBlock((hash, _)) => {
						log::info!(target: LOG_TARGET, "included at {:?}", hash);
						let key = StorageKey(
							frame_support::storage::storage_prefix(b"System", b"Events").to_vec(),
						);

						let events = match rpc.get_storage_and_decode::<
							Vec<frame_system::EventRecord<RuntimeEvent, <Block as BlockT>::Hash>>,
						>(&key, Some(hash))
						.await {
							Ok(rp) => rp.unwrap_or_default(),
							Err(RpcHelperError::JsonRpsee(RpcError::RestartNeeded(e))) => {
								let _ = tx.send(RpcError::RestartNeeded(e).into());
								return;
							}
							// Decoding or other RPC error => just terminate the task.
							Err(e) => {
								log::warn!(target: LOG_TARGET, "get_storage [key: {:?}, hash: {:?}] failed: {:?}; skip block: {}",
									key, hash, e, at.number
								);
								return;
							}
						};

						log::info!(target: LOG_TARGET, "events at inclusion {:?}", events);
					},
					TransactionStatus::Retracted(hash) => {
						log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
					},
					TransactionStatus::Finalized((hash, _)) => {
						log::info!(target: LOG_TARGET, "Finalized at {:?}", hash);
						break
					},
					_ => {
						log::warn!(
							target: LOG_TARGET,
							"Stopping listen due to other status {:?}",
							status_update
						);
						break
					},
				};
			}
		}
	}
}}}

monitor_cmd_for!(polkadot);
monitor_cmd_for!(kusama);
monitor_cmd_for!(westend);

#[cfg(test)]
pub mod tests {
	use super::*;

	#[test]
	fn score_passes_strategy_works() {
		let s = |x| sp_npos_elections::ElectionScore { minimal_stake: x, ..Default::default() };
		let two = Perbill::from_percent(2);

		// anything passes Always
		assert!(score_passes_strategy(s(0), s(0), SubmissionStrategy::Always));
		assert!(score_passes_strategy(s(5), s(0), SubmissionStrategy::Always));
		assert!(score_passes_strategy(s(5), s(10), SubmissionStrategy::Always));

		// if leading
		assert!(score_passes_strategy(s(0), s(0), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(1), s(0), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(2), s(0), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(5), s(10), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(9), s(10), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(10), s(10), SubmissionStrategy::IfLeading));

		// if better by 2%
		assert!(!score_passes_strategy(s(50), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(!score_passes_strategy(s(100), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(!score_passes_strategy(s(101), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(!score_passes_strategy(s(102), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(score_passes_strategy(s(103), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(score_passes_strategy(s(150), s(100), SubmissionStrategy::ClaimBetterThan(two)));

		// if no less than 2% worse
		assert!(!score_passes_strategy(s(50), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(!score_passes_strategy(s(97), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(98), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(99), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(100), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(101), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(102), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(103), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(150), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
	}
}
