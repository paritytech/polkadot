// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

use crate::error::PrecheckError;
use futures::channel::oneshot;

/// Result of PVF prechecking performed by the validation host.
pub type PrecheckResult = Result<(), PrecheckError>;

/// Transmission end used for sending the PVF prechecking result.
pub type PrecheckResultSender = oneshot::Sender<PrecheckResult>;

/// Status of the PVF prechecking. Only makes sense for requests whose
/// processing was initiated, i.e. a worker was previously assigned to it.
/// Queued requests are stored separately in the queue.
pub enum PrecheckStatus {
	/// A worker is currently busy with the prechecking.
	InProgress { waiting_for_response: Vec<PrecheckResultSender> },
	/// Cached result for processed requests.
	Done(PrecheckResult),
}
