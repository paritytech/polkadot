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

//! Helper method for RPC.

use super::*;
use jsonrpsee_ws_client::traits::Client;
pub(crate) use jsonrpsee_ws_client::v2::params::JsonRpcParams;

#[macro_export]
macro_rules! params {
	($($param:expr),*) => {
		{
			let mut __params = vec![];
			$(
				__params.push(serde_json::to_value($param).expect("json serialization infallible; qed."));
			)*
			$crate::rpc_helpers::JsonRpcParams::Array(__params)
		}
	};
	() => {
		$crate::rpc::JsonRpcParams::NoParams,
	}
}

/// Make the rpc request, returning `Ret`.
pub(crate) async fn rpc<'a, Ret: serde::de::DeserializeOwned>(
	client: &WsClient,
	method: &'a str,
	params: JsonRpcParams<'a>,
) -> Result<Ret, Error> {
	client.request::<Ret>(method, params).await.map_err(Into::into)
}

/// Make the rpc request, decode the outcome into `Dec`. Don't use for storage, it will fail for
/// non-existent storage items.
pub(crate) async fn rpc_decode<'a, Dec: codec::Decode>(
	client: &WsClient,
	method: &'a str,
	params: JsonRpcParams<'a>,
) -> Result<Dec, Error> {
	let bytes = rpc::<sp_core::Bytes>(client, method, params).await?;
	<Dec as codec::Decode>::decode(&mut &*bytes.0).map_err(Into::into)
}

/// Get the storage item.
pub(crate) async fn get_storage<'a, T: codec::Decode>(
	client: &WsClient,
	params: JsonRpcParams<'a>,
) -> Result<Option<T>, Error> {
	let maybe_bytes = rpc::<Option<sp_core::Bytes>>(client, "state_getStorage", params).await?;
	if let Some(bytes) = maybe_bytes {
		let decoded = <T as codec::Decode>::decode(&mut &*bytes.0)?;
		Ok(Some(decoded))
	} else {
		Ok(None)
	}
}

use codec::{EncodeLike, FullCodec};
use frame_support::storage::{StorageMap, StorageValue};
#[allow(unused)]
pub(crate) async fn get_storage_value_frame_v2<'a, V: StorageValue<T>, T: FullCodec, Hash>(
	client: &WsClient,
	maybe_at: Option<Hash>,
) -> Result<Option<V::Query>, Error>
where
	V::Query: codec::Decode,
	Hash: serde::Serialize,
{
	let key = <V as StorageValue<T>>::hashed_key();
	get_storage::<V::Query>(&client, params! { key, maybe_at }).await
}

#[allow(unused)]
pub(crate) async fn get_storage_map_frame_v2<
	'a,
	Hash,
	KeyArg: EncodeLike<K>,
	K: FullCodec,
	T: FullCodec,
	M: StorageMap<K, T>,
>(
	client: &WsClient,
	key: KeyArg,
	maybe_at: Option<Hash>,
) -> Result<Option<M::Query>, Error>
where
	M::Query: codec::Decode,
	Hash: serde::Serialize,
{
	let key = <M as StorageMap<K, T>>::hashed_key_for(key);
	get_storage::<M::Query>(&client, params! { key, maybe_at }).await
}
