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

use crate::{rpc_helpers, AccountId, Error, Index, Pair, WsClient, LOG_TARGET};
use sp_core::crypto::Pair as _;
use std::path::Path;

pub(crate) const SIGNER_ACCOUNT_WILL_EXIST: &'static str =
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
	/// The raw URI read from file.
	pub(crate) uri: String,
}

pub(crate) async fn get_account_info<T: frame_system::Config>(
	client: &WsClient,
	who: &T::AccountId,
	maybe_at: Option<T::Hash>,
) -> Result<Option<frame_system::AccountInfo<Index, T::AccountData>>, Error> {
	rpc_helpers::get_storage::<frame_system::AccountInfo<Index, T::AccountData>>(
		client,
		crate::params! {
			sp_core::storage::StorageKey(<frame_system::Account<T>>::hashed_key_for(&who)),
			maybe_at
		},
	)
	.await
}

/// Read the signer account's URI from the given `path`.
pub(crate) async fn read_signer_uri<
	P: AsRef<Path>,
	T: frame_system::Config<AccountId = AccountId, Index = Index>,
>(
	path: P,
	client: &WsClient,
) -> Result<Signer, Error> {
	let uri = std::fs::read_to_string(path)?;

	// trim any trailing garbage.
	let uri = uri.trim_end();

	let pair = Pair::from_string(&uri, None)?;
	let account = T::AccountId::from(pair.public());
	let _info = get_account_info::<T>(&client, &account, None)
		.await?
		.ok_or(Error::AccountDoesNotExists)?;
	log::info!(target: LOG_TARGET, "loaded account {:?}, info: {:?}", &account, _info);
	Ok(Signer { account, pair, uri: uri.to_string() })
}
