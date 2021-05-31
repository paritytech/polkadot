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

use crate::{prelude::*, rpc_helpers::*, params, Signer, SharedConfig, MonitorConfig, Error};
use jsonrpsee_ws_client::{
	traits::{SubscriptionClient},
	v2::params::JsonRpcParams,
	Subscription, WsClient,
};

/// Ensure that now is the singed phase.
async fn ensure_signed_phase<T: EPM::Config, B: BlockT>(
	client: &WsClient,
	at: B::Hash,
) -> Result<(), Error> {
	let key = sp_core::storage::StorageKey(EPM::CurrentPhase::<T>::hashed_key().to_vec());
	let phase = get_storage::<EPM::Phase<BlockNumber>>(client, params! {key, at}).await?.unwrap_or_default();

	if phase.is_signed() {
		Ok(())
	} else {
		Err(Error::IncorrectPhase)
	}
}

/// Ensure that our current `us` have not submitted anything previously.
async fn ensure_no_previous_solution<T: EPM::Config, B: BlockT>(
	client: &WsClient,
	at: B::Hash,
	us: &AccountId,
) -> Result<(), Error> {
	use EPM::{SignedSubmissions, signed::SignedSubmission};
	let key = sp_core::storage::StorageKey(SignedSubmissions::<T>::hashed_key().to_vec());
	let queue = crate::any_runtime! {
		get_storage::<Vec<SignedSubmission<AccountId, Balance, NposCompactSolution>>>(client, params!{ key, at }).await
	}?.unwrap_or_default();

	// if we have a solution in the queue, then don't do anything.
	if queue.iter().any(|ss| &ss.who == us) {
		Err(Error::AlreadySubmitted)
	} else {
		Ok(())
	}
}

macro_rules! monitor_cmd_for { ($runtime:tt) => { paste::paste! {
	/// The monitor command.
	pub(crate) async fn [<monitor_cmd_ $runtime>](
		client: WsClient,
		shared: SharedConfig,
		config: MonitorConfig,
		signer: Signer, // TODO: Signer could also go into a shared config, perhaps.
	) -> Result<(), Error> {
		use $crate::[<$runtime _runtime_exports>]::*;
		let subscription_method = if config.listen == "head" {
			"chain_subscribeNewHeads"
		} else {
			"chain_subscribeFinalizedHeads"
		};

		log::info!(target: LOG_TARGET, "subscribing to {:?}", subscription_method);
		let mut subscription: Subscription<Header> = client
			.subscribe(&subscription_method, JsonRpcParams::NoParams, "unsubscribe")
			.await
			.unwrap();

		loop {
			let now = subscription.next().await.unwrap();
			let hash = now.hash();
			log::debug!(target: LOG_TARGET, "new event at #{:?} ({:?})", now.number, hash);

			if ensure_signed_phase::<Runtime, Block>(&client, hash).await.is_err() {
				log::debug!(target: LOG_TARGET, "phase closed, not interested in this block at all.");
				continue;
			};

			if ensure_no_previous_solution::<Runtime, Block>(&client, hash, &signer.account).await.is_err()
			{
				log::debug!(target: LOG_TARGET, "We already have a solution in this phase, skipping.");
				continue;
			}

			// grab an externalities without staking, just the election snapshot.
			let mut ext = crate::create_election_ext::<Runtime, Block>(shared.uri.clone(), hash, false).await;
			let (raw_solution, witness) = crate::mine_unchecked::<Runtime>(&mut ext);
			log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

			let extrinsic = ext.execute_with(|| create_uxt(raw_solution, witness, signer.clone()));
		}
	}
}}}

monitor_cmd_for!(polkadot);
monitor_cmd_for!(kusama);
monitor_cmd_for!(westend);

// This is my best WIP to make ^^ generic.. it won't work.
// macro_rules! monitor_cmd_for { ($runtime:tt) => { paste::paste! {
// 	pub(crate) async fn monitor_cmd_generic<T, B, C, E, X>(
// 		client: WsClient,
// 		shared: SharedConfig,
// 		config: MonitorConfig,
// 		signer: Signer,
// 	) -> Result<(), Error>
// 		where
// 		B::Header: serde::de::DeserializeOwned,
// 		T: EPM::Config + frame_system::Config<AccountId = AccountId>,
// 		B: BlockT,
// 		C: codec::Encode + sp_runtime::traits::Member + From<EPM::Call<T>>,
// 		X: sp_runtime::traits::SignedExtension,
// 		E: sp_runtime::traits::Extrinsic<
// 			Call = C,
// 			SignaturePayload = (
// 				<<T as frame_system::Config>::Lookup as sp_runtime::traits::StaticLookup>::Source,
// 				Signature,
// 				X,
// 			)
// 		>,
// 	{
// 		let subscription_method = if config.listen == "heads" {
// 			"chain_subscribeNewHeads"
// 		} else {
// 			"chain_subscribeFinalizedHeads"
// 		};

// 		log::info!(target: LOG_TARGET, "subscribing to {:?}", subscription_method);
// 		let mut subscription: Subscription<B::Header> = client
// 			.subscribe(&subscription_method, JsonRpcParams::NoParams, "unsubscribe")
// 			.await
// 			.unwrap();

// 		loop {
// 			let now = subscription.next().await.unwrap();
// 			let hash = now.hash();
// 			log::debug!(target: LOG_TARGET, "new event at #{:?} ({:?})", now.number(), hash);

// 			if ensure_signed_phase::<T, B>(&client, hash).await.is_err() {
// 				log::debug!(target: LOG_TARGET, "phase closed, not interested in this block at all.");
// 				continue;
// 			};

// 			if ensure_no_previous_solution::<T, B>(&client, hash, &signer.account).await.is_err()
// 			{
// 				log::debug!(target: LOG_TARGET, "We already have a solution in this phase, skipping.");
// 				continue;
// 			}

// 			// mine a new solution, with the exact configuration of the on-chain stuff. No need to fetch
// 			// staking now, the snapshot must exist.
// 			if ensure_snapshot_exists::<T, B>(&client, hash).await.is_err() {
// 				log::error!(target: LOG_TARGET, "Phase is signed, but snapshot is not there.");
// 				continue;
// 			}

// 			// grab an externalities without staking, just the election snapshot.
// 			let mut ext = crate::create_election_ext::<T, B>(shared.uri.clone(), hash, false).await;
// 			let (raw_solution, witness) = crate::mine_unchecked::<T>(&mut ext);
// 			log::info!(target: LOG_TARGET, "mined solution with {:?}", &raw_solution.score);

// 			let call = EPM::Call::<T>::submit(raw_solution, witness);
// 			let outer_call: C = call.into();

// 			// ------------------
// 			let crate::Signer { account, pair, .. } = signer.clone();
// 			let nonce = 0; // TODO
// 			let tip = 0;
// 			let extra = crate::any_runtime! { get_signed_extra::<X>() };
// 			let raw_payload = sp_runtime::generic::SignedPayload::new(outer_call.clone(),
// extra.clone()).unwrap(); 			let signature = raw_payload.using_encoded(|payload| {
// 				pair.clone().sign(payload)
// 			});
// 			let address = <T as frame_system::Config>::Lookup::unlookup(account.clone());
// 			let extrinsic = E::new(outer_call, Some((address, signature.into(), extra))).unwrap();
// 		}
// 	}
// }}}
