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

//! Wrappers around creating a signer account.

use crate::{prelude::*, rpc_helpers, AccountId, Error, Index, Pair, WsClient, LOG_TARGET};
use sp_core::crypto::Pair as _;

pub(crate) const SIGNER_ACCOUNT_WILL_EXIST: &str =
	"signer account is checked to exist upon startup; it can only die if it transfers funds out \
	 of it, or get slashed. If it does not exist at this point, it is likely due to a bug, or the \
	 signer got slashed. Terminating.";

/// Some information about the signer. Redundant at this point, but makes life easier.
#[derive(Clone)]
pub(crate) struct Signer {
	/// The account id.
	pub(crate) account: AccountId,

	/// The full crypto key-pair.
	pub(crate) pair: Pair,
}

pub(crate) async fn get_account_info<T: frame_system::Config + EPM::Config>(
	client: &WsClient,
	who: &T::AccountId,
	maybe_at: Option<T::Hash>,
) -> Result<Option<frame_system::AccountInfo<Index, T::AccountData>>, Error<T>> {
	rpc_helpers::get_storage::<frame_system::AccountInfo<Index, T::AccountData>>(
		client,
		jsonrpsee::rpc_params! {
			sp_core::storage::StorageKey(<frame_system::Account<T>>::hashed_key_for(&who)),
			maybe_at
		},
	)
	.await
	.map_err(Into::into)
}

/// Read the signer account's URI
pub(crate) async fn signer_uri_from_string<
	T: frame_system::Config<
			AccountId = AccountId,
			Index = Index,
			AccountData = pallet_balances::AccountData<Balance>,
		> + EPM::Config,
>(
	seed: &str,
	client: &WsClient,
) -> Result<Signer, Error<T>> {
	let seed = seed.trim();

	let pair = Pair::from_string(seed, None)?;
	let account = T::AccountId::from(pair.public());
	let _info = get_account_info::<T>(client, &account, None)
		.await?
		.ok_or(Error::<T>::AccountDoesNotExists)?;
	log::info!(
		target: LOG_TARGET,
		"loaded account {:?}, free: {:?}, info: {:?}",
		&account,
		Token::from(_info.data.free),
		_info
	);
	Ok(Signer { account, pair })
}
