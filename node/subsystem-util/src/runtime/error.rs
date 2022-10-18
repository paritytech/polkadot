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
//

//! Error handling related code and Error/Result definitions.

use futures::channel::oneshot;

use polkadot_node_subsystem::errors::RuntimeApiError;
use polkadot_primitives::v2::SessionIndex;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	/// Runtime API subsystem is down, which means we're shutting down.
	#[fatal]
	#[error("Runtime request got canceled")]
	RuntimeRequestCanceled(oneshot::Canceled),

	/// Some request to the runtime failed.
	/// For example if we prune a block we're requesting info about.
	#[error("Runtime API error {0}")]
	RuntimeRequest(RuntimeApiError),

	/// We tried fetching a session info which was not available.
	#[error("There was no session with the given index {0}")]
	NoSuchSession(SessionIndex),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Receive a response from a runtime request and convert errors.
pub(crate) async fn recv_runtime<V>(
	r: oneshot::Receiver<std::result::Result<V, RuntimeApiError>>,
) -> Result<V> {
	let result = r
		.await
		.map_err(FatalError::RuntimeRequestCanceled)?
		.map_err(JfyiError::RuntimeRequest)?;
	Ok(result)
}
