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
	params, prelude::*, rpc_helpers::*, signer::Signer, Error, MonitorConfig, SharedConfig,
};
use codec::Encode;
use jsonrpsee_ws_client::{
	WsClient,
	types::{Subscription, traits::SubscriptionClient, v2::params::JsonRpcParams},
};
use sc_transaction_pool_api::TransactionStatus;

/// Ensure that now is the signed phase.
async fn ensure_signed_phase<T: EPM::Config, B: BlockT>(
	client: &WsClient,
	at: B::Hash,
) -> Result<(), Error> {
	let key = sp_core::storage::StorageKey(EPM::CurrentPhase::<T>::hashed_key().to_vec());
	let phase = get_storage::<EPM::Phase<BlockNumber>>(client, params! {key, at})
		.await?
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
) -> Result<(), Error> {
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
		client: &WsClient,
		shared: SharedConfig,
		config: MonitorConfig,
		signer: Signer,
	) -> Result<(), Error> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let (sub, unsub) = if config.listen == "head" {
			("chain_subscribeNewHeads", "chain_unsubscribeNewHeads")
		} else {
			("chain_subscribeFinalizedHeads", "chain_unsubscribeFinalizedHeads")
		};

		log::info!(target: LOG_TARGET, "subscribing to {:?} / {:?}", sub, unsub);
		let mut subscription: Subscription<Header> = client
			.subscribe(&sub, JsonRpcParams::NoParams, &unsub)
			.await
			.unwrap();

		while let Some(now) = subscription.next().await? {
			let hash = now.hash();
			log::debug!(target: LOG_TARGET, "new event at #{:?} ({:?})", now.number, hash);

			// we prefer doing this check before fetching anything into a remote-ext.
			if ensure_signed_phase::<Runtime, Block>(client, hash).await.is_err() {
				log::debug!(target: LOG_TARGET, "phase closed, not interested in this block at all.");
				continue;
			};

			// NOTE: we don't check the score of any of the submitted solutions. If we submit a weak
			// one, as long as we are valid, we will end up getting our deposit back, so not a big
			// deal for now. Note that to avoid an unfeasible solution, we should make sure that we
			// only start the process on a finalized snapshot. If the signed phase is long enough,
			// this will not be a solution.

			// grab an externalities without staking, just the election snapshot.
			let mut ext = crate::create_election_ext::<Runtime, Block>(shared.uri.clone(), Some(hash), false).await?;

			if ensure_no_previous_solution::<Runtime, Block>(&mut ext, &signer.account).await.is_err() {
				log::debug!(target: LOG_TARGET, "We already have a solution in this phase, skipping.");
				continue;
			}

			let (raw_solution, witness) = crate::mine_unchecked::<Runtime>(&mut ext, config.iterations, true)?;
			log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

			let nonce = crate::get_account_info::<Runtime>(client, &signer.account, Some(hash))
				.await?
				.map(|i| i.nonce)
				.expect(crate::signer::SIGNER_ACCOUNT_WILL_EXIST);
			let tip = 0 as Balance;
			let period = <Runtime as frame_system::Config>::BlockHashCount::get() / 2;
			let current_block = now.number.saturating_sub(1);
			let era = sp_runtime::generic::Era::mortal(period.into(), current_block.into());
			log::trace!(target: LOG_TARGET, "transaction mortality: {:?} -> {:?}", era.birth(current_block.into()), era.death(current_block.into()));
			let extrinsic = ext.execute_with(|| create_uxt(raw_solution, witness, signer.clone(), nonce, tip, era));
			let bytes = sp_core::Bytes(extrinsic.encode());

			let mut tx_subscription: Subscription<
				TransactionStatus<<Block as BlockT>::Hash, <Block as BlockT>::Hash>
			> = match client
				.subscribe(&"author_submitAndWatchExtrinsic", params! { bytes }, "author_unwatchExtrinsic")
				.await
			{
				Ok(sub) => sub,
				Err(why) => {
				// This usually happens when we've been busy with mining for a few blocks, and now we're receiving the
				// subscriptions of blocks in which we were busy. In these blocks, we still don't have a solution, so we
				// re-compute a new solution and submit it with an outdated `Nonce`, which yields most often `Stale`
				// error. NOTE: to improve this overall, and to be able to introduce an array of other fancy features,
				// we should make this multi-threaded and do the computation outside of this callback.
					log::warn!(target: LOG_TARGET, "failing to submit a transaction {:?}. continuing...", why);
					continue
				}
			};

			let _success = while let Some(status_update) = tx_subscription.next().await? {
				log::trace!(target: LOG_TARGET, "status update {:?}", status_update);
				match status_update {
					TransactionStatus::Ready | TransactionStatus::Broadcast(_) | TransactionStatus::Future => continue,
					TransactionStatus::InBlock(hash) => {
						log::info!(target: LOG_TARGET, "included at {:?}", hash);
						let key = sp_core::storage::StorageKey(frame_system::Events::<Runtime>::hashed_key().to_vec());
						let events =get_storage::<
							Vec<frame_system::EventRecord<Event, <Block as BlockT>::Hash>>
						>(client, params!{ key, hash }).await?.unwrap_or_default();
						log::info!(target: LOG_TARGET, "events at inclusion {:?}", events);
					}
					TransactionStatus::Retracted(hash) => {
						log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
					}
					TransactionStatus::Finalized(hash) => {
						log::info!(target: LOG_TARGET, "Finalized at {:?}", hash);
						break
					}
					_ => {
						log::warn!(target: LOG_TARGET, "Stopping listen due to other status {:?}", status_update);
						break
					}
				}
			};
		}

		Ok(())
	}
}}}

monitor_cmd_for!(polkadot);
monitor_cmd_for!(kusama);
monitor_cmd_for!(westend);
