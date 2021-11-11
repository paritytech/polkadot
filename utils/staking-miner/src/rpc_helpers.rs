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
use jsonrpsee::types::traits::Client;
pub(crate) use jsonrpsee::types::v2::ParamsSer;

#[derive(frame_support::DebugNoBound, thiserror::Error)]
pub(crate) enum RpcHelperError {
	JsonRpsee(#[from] jsonrpsee::types::Error),
	Codec(#[from] codec::Error),
}

impl std::fmt::Display for RpcHelperError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		<RpcHelperError as std::fmt::Debug>::fmt(self, f)
	}
}

/// Make the rpc request, returning `Ret`.
pub(crate) async fn rpc<'a, Ret: serde::de::DeserializeOwned>(
	client: &WsClient,
	method: &'a str,
	params: Option<ParamsSer<'a>>,
) -> Result<Ret, RpcHelperError> {
	client
		.request::<Ret>(method, params)
		.await
		.map_err::<RpcHelperError, _>(Into::into)
}

/// Make the rpc request, decode the outcome into `Dec`. Don't use for storage, it will fail for
/// non-existent storage items.
pub(crate) async fn rpc_decode<'a, Dec: codec::Decode>(
	client: &WsClient,
	method: &'a str,
	params: Option<ParamsSer<'a>>,
) -> Result<Dec, RpcHelperError> {
	let bytes = rpc::<sp_core::Bytes>(client, method, params)
		.await
		.map_err::<RpcHelperError, _>(Into::into)?;
	<Dec as codec::Decode>::decode(&mut &*bytes.0).map_err::<RpcHelperError, _>(Into::into)
}

/// Get the storage item.
pub(crate) async fn get_storage<'a, T: codec::Decode>(
	client: &WsClient,
	params: Option<ParamsSer<'a>>,
) -> Result<Option<T>, RpcHelperError> {
	let maybe_bytes = rpc::<Option<sp_core::Bytes>>(client, "state_getStorage", params)
		.await
		.map_err::<RpcHelperError, _>(Into::into)?;
	if let Some(bytes) = maybe_bytes {
		let decoded = <T as codec::Decode>::decode(&mut &*bytes.0)
			.map_err::<RpcHelperError, _>(Into::into)?;
		Ok(Some(decoded))
	} else {
		Ok(None)
	}
}
