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

use crate::{prelude::*, rpc::*, signer::Signer, Error, MonitorConfig, SharedRpcClient};
use codec::Encode;
use jsonrpsee::core::Error as RpcError;
use sc_transaction_pool_api::TransactionStatus;
use sp_core::storage::StorageKey;
use tokio::sync::mpsc;

/// Ensure that now is the signed phase.
async fn ensure_signed_phase<T: EPM::Config, B: BlockT<Hash = Hash>>(
	rpc: &SharedRpcClient,
	at: B::Hash,
) -> Result<(), Error<T>> {
	let key = StorageKey(EPM::CurrentPhase::<T>::hashed_key().to_vec());
	let phase = rpc
		.get_storage::<EPM::Phase<BlockNumber>>(key, Some(at))
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
async fn ensure_no_previous_solution<
	T: EPM::Config + frame_system::Config<AccountId = AccountId>,
	B: BlockT,
>(
	ext: &mut Ext,
	us: &AccountId,
) -> Result<(), Error<T>> {
	use EPM::signed::SignedSubmissions;
	ext.execute_with(|| {
		if <SignedSubmissions<T>>::get().iter().any(|ss| &ss.who == us) {
			Err(Error::AlreadySubmitted)
		} else {
			Ok(())
		}
	})
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
						// Custom `jsonrpsee` message sent by the server if the subscription was closed on the server side.
						Some(Err(RpcError::SubscriptionClosed(reason))) => {
							log::warn!(target: LOG_TARGET, "subscription to chain_subscribeHeads/FinalizedHeads terminated: {:?}. Retrying..", reason);
							subscription = heads_subscription().await?;
							continue;
						}
						Some(Err(e)) => {
							log::error!(target: LOG_TARGET, "subscription failed to decode Header {:?}, this is bug please file an issue", e);
							return Err(e.into());
						}
						// The subscription was dropped, should only happen if:
						//	- the connection was closed.
						//	- the subscription could not keep up with the server.
						None => {
							log::warn!(target: LOG_TARGET, "subscription to chain_subscribeHeads/FinalizedHeads terminated. Retrying..");
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

			let hash = at.hash();
			log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

			// if the runtime version has changed, terminate.
			if let Err(err) = crate::check_versions::<Runtime>(&rpc).await {
				let _ = tx.send(err.into());
				return;
			}

			// we prefer doing this check before fetching anything into a remote-ext.
			if ensure_signed_phase::<Runtime, Block>(&rpc, hash).await.is_err() {
				log::debug!(target: LOG_TARGET, "phase closed, not interested in this block at all.");
				return;
			}

			// grab an externalities without staking, just the election snapshot.
			let mut ext = match crate::create_election_ext::<Runtime, Block>(
				rpc.clone(),
				Some(hash),
				vec![],
			).await {
				Ok(ext) => ext,
				Err(err) => {
					let _ = tx.send(err);
					return;
				}
			};

			if ensure_no_previous_solution::<Runtime, Block>(&mut ext, &signer.account).await.is_err() {
				log::debug!(target: LOG_TARGET, "We already have a solution in this phase, skipping.");
				return;
			}

			// mine a solution, and run feasibility check on it as well.
			let (raw_solution, witness) = match crate::mine_with::<Runtime>(&config.solver, &mut ext, true) {
				Ok(r) => r,
				Err(err) => {
					let _ = tx.send(err.into());
					return;
				}
			};

			log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

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

			let extrinsic = ext.execute_with(|| create_uxt(raw_solution, witness, signer.clone(), nonce, tip, era));
			let bytes = sp_core::Bytes(extrinsic.encode());

			let mut tx_subscription = match rpc.watch_extrinsic(bytes).await {
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
					// Custom `jsonrpsee` message sent by the server if the subscription was closed on the server side.
					Err(RpcError::SubscriptionClosed(reason)) => {
						log::warn!(
							target: LOG_TARGET,
							"tx subscription closed by the server: {:?}; skip block: {}",
							reason, at.number
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
					TransactionStatus::Ready |
					TransactionStatus::Broadcast(_) |
					TransactionStatus::Future => continue,
					TransactionStatus::InBlock(hash) => {
						log::info!(target: LOG_TARGET, "included at {:?}", hash);
						let key = StorageKey(
							frame_support::storage::storage_prefix(b"System", b"Events").to_vec(),
						);
						let key2 = key.clone();

						let events = match rpc.get_storage::<
							Vec<frame_system::EventRecord<Event, <Block as BlockT>::Hash>>,
						>(key, Some(hash))
						.await {
							Ok(rp) => rp.unwrap_or_default(),
							Err(RpcHelperError::JsonRpsee(RpcError::RestartNeeded(e))) => {
								let _ = tx.send(RpcError::RestartNeeded(e).into());
								return;
							}
							// Decoding or other RPC error => just terminate the task.
							Err(e) => {
								log::warn!(target: LOG_TARGET, "get_storage [key: {:?}, hash: {:?}] failed: {:?}; skip block: {}",
									key2, hash, e, at.number
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
