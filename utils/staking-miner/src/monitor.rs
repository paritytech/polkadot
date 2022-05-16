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

	for (_score, idx) in indices {
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

/// Reads all current solutions and checks the scores according to the `SubmissionStrategy`.
async fn ensure_no_better_solution<T: EPM::Config, B: BlockT>(
	rpc: &SharedRpcClient,
	at: Hash,
	score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
) -> Result<(), Error<T>> {
	let epsilon = match strategy {
		// don't care about current scores.
		SubmissionStrategy::Always => return Ok(()),
		SubmissionStrategy::IfLeading => Perbill::zero(),
		SubmissionStrategy::ClaimBetterThan(epsilon) => epsilon,
	};

	let indices_key = StorageKey(EPM::SignedSubmissionIndices::<T>::hashed_key().to_vec());

	let indices: SubmissionIndicesOf<T> = rpc
		.get_storage_and_decode(&indices_key, Some(at))
		.await
		.map_err::<Error<T>, _>(Into::into)?
		.unwrap_or_default();

	// BTreeMap is ordered, take last to get the max score.
	if let Some(curr_max_score) = indices.into_iter().last().map(|(s, _)| s) {
		if !score.strict_threshold_better(curr_max_score, epsilon) {
			return Err(Error::StrategyNotSatisfied)
		}
	}

	Ok(())
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
			if let Err(err) = crate::check_versions::<Runtime>(&rpc).await {
				let _ = tx.send(err.into());
				return;
			}

			let rpc1 = rpc.clone();
			let rpc2 = rpc.clone();
			let account = signer.account.clone();

			let signed_phase_fut = tokio::spawn(async move {
				ensure_signed_phase::<Runtime, Block>(&rpc1, hash).await
			});

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

			let ensure_no_better_fut = tokio::spawn(async move {
				ensure_no_better_solution::<Runtime, Block>(&rpc1, hash, score, config.submission_strategy).await
			});

			let ensure_signed_phase_fut = tokio::spawn(async move {
				ensure_signed_phase::<Runtime, Block>(&rpc2, hash).await
			});

			// Run the calls in parallel and return once all has completed or any failed.
			if tokio::try_join!(
				flatten(ensure_no_better_fut),
				flatten(ensure_signed_phase_fut),
			).is_err() {
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
					TransactionStatus::InBlock(hash) => {
						log::info!(target: LOG_TARGET, "included at {:?}", hash);
						let key = StorageKey(
							frame_support::storage::storage_prefix(b"System", b"Events").to_vec(),
						);

						let events = match rpc.get_storage_and_decode::<
							Vec<frame_system::EventRecord<Event, <Block as BlockT>::Hash>>,
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
					TransactionStatus::Finalized(hash) => {
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
