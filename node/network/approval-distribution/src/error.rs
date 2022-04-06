// Copyright 2022 Parity Technologies (UK) Ltd.
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
use polkadot_node_subsystem::SubsystemError;

/// General result.
pub type Result<T> = std::result::Result<T, Error>;
/// Result for non-fatal only failures.
pub type JfyiErrorResult<T> = std::result::Result<T, JfyiError>;
/// Result for fatal only failures.
pub type FatalResult<T> = std::result::Result<T, FatalError>;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal]
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	#[fatal]
	#[error("Receiving message from overseer failed")]
	SubsystemReceive(#[source] SubsystemError),
}
