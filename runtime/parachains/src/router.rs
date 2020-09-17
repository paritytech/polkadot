// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The router module is responsible for handling messaging.
//!
//! The core of the messaging is checking and processing messages sent out by the candidates,
//! routing the messages at their destinations and informing the parachains about the incoming
//! messages.

use crate::{
	configuration,
	initializer,
};
use sp_std::prelude::*;
use frame_support::{decl_error, decl_module, decl_storage, weights::Weight};
use sp_std::collections::vec_deque::VecDeque;
use primitives::v1::{Id as ParaId, InboundDownwardMessage, Hash, UpwardMessage};

mod dmp;
mod ump;

pub use dmp::QueueDownwardMessageError;
pub use ump::UmpSink;

#[cfg(test)]
pub use ump::mock_sink::MockUmpSink;

pub trait Trait: frame_system::Trait + configuration::Trait {
	/// A place where all received upward messages are funneled.
	type UmpSink: UmpSink;
}

decl_storage! {
	trait Store for Module<T: Trait> as Router {
		/// Paras that are to be cleaned up at the end of the session.
		/// The entries are sorted ascending by the para id.
		OutgoingParas: Vec<ParaId>;

		/*
		 * Downward Message Passing (DMP)
		 *
		 * Storage layout required for implementation of DMP.
		 */

		/// The downward messages addressed for a certain para.
		DownwardMessageQueues: map hasher(twox_64_concat) ParaId => Vec<InboundDownwardMessage<T::BlockNumber>>;
		/// A mapping that stores the downward message queue MQC head for each para.
		///
		/// Each link in this chain has a form:
		/// `(prev_head, B, H(M))`, where
		/// - `prev_head`: is the previous head hash or zero if none.
		/// - `B`: is the relay-chain block number in which a message was appended.
		/// - `H(M)`: is the hash of the message being appended.
		DownwardMessageQueueHeads: map hasher(twox_64_concat) ParaId => Hash;

		/*
		 * Upward Message Passing (UMP)
		 *
		 * Storage layout required for UMP, specifically dispatchable upward messages.
		 */

		/// The messages waiting to be handled by the relay-chain originating from a certain parachain.
		///
		/// Note that some upward messages might have been already processed by the inclusion logic. E.g.
		/// channel management messages.
		///
		/// The messages are processed in FIFO order.
		RelayDispatchQueues: map hasher(twox_64_concat) ParaId => VecDeque<UpwardMessage>;
		/// Size of the dispatch queues. Caches sizes of the queues in `RelayDispatchQueue`.
		///
		/// First item in the tuple is the count of messages and second
		/// is the total length (in bytes) of the message payloads.
		///
		/// Note that this is an auxilary mapping: it's possible to tell the byte size and the number of
		/// messages only looking at `RelayDispatchQueues`. This mapping is separate to avoid the cost of
		/// loading the whole message queue if only the total size and count are required.
		///
		/// Invariant:
		/// - The set of keys should exactly match the set of keys of `RelayDispatchQueues`.
		RelayDispatchQueueSize: map hasher(twox_64_concat) ParaId => (u32, u32);
		/// The ordered list of `ParaId`s that have a `RelayDispatchQueue` entry.
		///
		/// Invariant:
		/// - The set of items from this vector should be exactly the set of the keys in
		///   `RelayDispatchQueues` and `RelayDispatchQueueSize`.
		NeedsDispatch: Vec<ParaId>;
		/// This is the para that gets will get dispatched first during the next upward dispatchable queue
		/// execution round.
		///
		/// Invariant:
		/// - If `Some(para)`, then `para` must be present in `NeedsDispatch`.
		NextDispatchRoundStartWith: Option<ParaId>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The router module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {
	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(
		_notification: &initializer::SessionChangeNotification<T::BlockNumber>,
	) {
		let outgoing = OutgoingParas::take();
		for outgoing_para in outgoing {
			Self::clean_dmp_after_outgoing(outgoing_para);
			Self::clean_ump_after_outgoing(outgoing_para);
		}
	}

	/// Schedule a para to be cleaned up at the start of the next session.
	pub fn schedule_para_cleanup(id: ParaId) {
		OutgoingParas::mutate(|v| {
			if let Err(i) = v.binary_search(&id) {
				v.insert(i, id);
			}
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use primitives::v1::BlockNumber;
	use frame_support::traits::{OnFinalize, OnInitialize};

	use crate::mock::{System, Router, GenesisConfig as MockGenesisConfig};

	pub(crate) fn run_to_block(to: BlockNumber, new_session: Option<Vec<BlockNumber>>) {
		while System::block_number() < to {
			let b = System::block_number();
			Router::initializer_finalize();
			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			if new_session.as_ref().map_or(false, |v| v.contains(&(b + 1))) {
				Router::initializer_on_new_session(&Default::default());
			}
			Router::initializer_initialize(b + 1);
		}
	}

	pub(crate) fn default_genesis_config() -> MockGenesisConfig {
		MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: crate::configuration::HostConfiguration {
					max_downward_message_size: 1024,
					..Default::default()
				},
			},
			..Default::default()
		}
	}
}
