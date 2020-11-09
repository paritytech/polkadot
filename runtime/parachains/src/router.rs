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

use crate::{configuration, paras, initializer, ensure_parachain};
use sp_std::prelude::*;
use frame_support::{decl_error, decl_module, decl_storage, dispatch::DispatchResult, weights::Weight};
use sp_std::collections::vec_deque::VecDeque;
use primitives::v1::{
	Id as ParaId, InboundDownwardMessage, Hash, UpwardMessage, HrmpChannelId, InboundHrmpMessage,
};

mod dmp;
mod hrmp;
mod ump;

use hrmp::{HrmpOpenChannelRequest, HrmpChannel};
pub use dmp::QueueDownwardMessageError;
pub use ump::UmpSink;

#[cfg(test)]
pub use ump::mock_sink::MockUmpSink;

pub trait Trait: frame_system::Trait + configuration::Trait + paras::Trait {
	type Origin: From<crate::Origin>
		+ From<<Self as frame_system::Trait>::Origin>
		+ Into<Result<crate::Origin, <Self as Trait>::Origin>>;

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

		/*
		 * Horizontally Relay-routed Message Passing (HRMP)
		 *
		 * HRMP related storage layout
		 */

		/// The set of pending HRMP open channel requests.
		///
		/// The set is accompanied by a list for iteration.
		///
		/// Invariant:
		/// - There are no channels that exists in list but not in the set and vice versa.
		HrmpOpenChannelRequests: map hasher(twox_64_concat) HrmpChannelId => Option<HrmpOpenChannelRequest>;
		HrmpOpenChannelRequestsList: Vec<HrmpChannelId>;

		/// This mapping tracks how many open channel requests are inititated by a given sender para.
		/// Invariant: `HrmpOpenChannelRequests` should contain the same number of items that has `(X, _)`
		/// as the number of `HrmpOpenChannelRequestCount` for `X`.
		HrmpOpenChannelRequestCount: map hasher(twox_64_concat) ParaId => u32;
		/// This mapping tracks how many open channel requests were accepted by a given recipient para.
		/// Invariant: `HrmpOpenChannelRequests` should contain the same number of items `(_, X)` with
		/// `confirmed` set to true, as the number of `HrmpAcceptedChannelRequestCount` for `X`.
		HrmpAcceptedChannelRequestCount: map hasher(twox_64_concat) ParaId => u32;

		/// A set of pending HRMP close channel requests that are going to be closed during the session change.
		/// Used for checking if a given channel is registered for closure.
		///
		/// The set is accompanied by a list for iteration.
		///
		/// Invariant:
		/// - There are no channels that exists in list but not in the set and vice versa.
		HrmpCloseChannelRequests: map hasher(twox_64_concat) HrmpChannelId => Option<()>;
		HrmpCloseChannelRequestsList: Vec<HrmpChannelId>;

		/// The HRMP watermark associated with each para.
		/// Invariant:
		/// - each para `P` used here as a key should satisfy `Paras::is_valid_para(P)` within a session.
		HrmpWatermarks: map hasher(twox_64_concat) ParaId => Option<T::BlockNumber>;
		/// HRMP channel data associated with each para.
		/// Invariant:
		/// - each participant in the channel should satisfy `Paras::is_valid_para(P)` within a session.
		HrmpChannels: map hasher(twox_64_concat) HrmpChannelId => Option<HrmpChannel>;
		/// Ingress/egress indexes allow to find all the senders and receivers given the opposite
		/// side. I.e.
		///
		/// (a) ingress index allows to find all the senders for a given recipient.
		/// (b) egress index allows to find all the recipients for a given sender.
		///
		/// Invariants:
		/// - for each ingress index entry for `P` each item `I` in the index should present in `HrmpChannels`
		///   as `(I, P)`.
		/// - for each egress index entry for `P` each item `E` in the index should present in `HrmpChannels`
		///   as `(P, E)`.
		/// - there should be no other dangling channels in `HrmpChannels`.
		/// - the vectors are sorted.
		HrmpIngressChannelsIndex: map hasher(twox_64_concat) ParaId => Vec<ParaId>;
		HrmpEgressChannelsIndex: map hasher(twox_64_concat) ParaId => Vec<ParaId>;
		/// Storage for the messages for each channel.
		/// Invariant: cannot be non-empty if the corresponding channel in `HrmpChannels` is `None`.
		HrmpChannelContents: map hasher(twox_64_concat) HrmpChannelId => Vec<InboundHrmpMessage<T::BlockNumber>>;
		/// Maintains a mapping that can be used to answer the question:
		/// What paras sent a message at the given block number for a given reciever.
		/// Invariants:
		/// - The inner `Vec<ParaId>` is never empty.
		/// - The inner `Vec<ParaId>` cannot store two same `ParaId`.
		/// - The outer vector is sorted ascending by block number and cannot store two items with the same
		///   block number.
		HrmpChannelDigests: map hasher(twox_64_concat) ParaId => Vec<(T::BlockNumber, Vec<ParaId>)>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// The sender tried to open a channel to themselves.
		OpenHrmpChannelToSelf,
		/// The recipient is not a valid para.
		OpenHrmpChannelInvalidRecipient,
		/// The requested capacity is zero.
		OpenHrmpChannelZeroCapacity,
		/// The requested capacity exceeds the global limit.
		OpenHrmpChannelCapacityExceedsLimit,
		/// The requested maximum message size is 0.
		OpenHrmpChannelZeroMessageSize,
		/// The open request requested the message size that exceeds the global limit.
		OpenHrmpChannelMessageSizeExceedsLimit,
		/// The channel already exists
		OpenHrmpChannelAlreadyExists,
		/// There is already a request to open the same channel.
		OpenHrmpChannelAlreadyRequested,
		/// The sender already has the maximum number of allowed outbound channels.
		OpenHrmpChannelLimitExceeded,
		/// The channel from the sender to the origin doesn't exist.
		AcceptHrmpChannelDoesntExist,
		/// The channel is already confirmed.
		AcceptHrmpChannelAlreadyConfirmed,
		/// The recipient already has the maximum number of allowed inbound channels.
		AcceptHrmpChannelLimitExceeded,
		/// The origin tries to close a channel where it is neither the sender nor the recipient.
		CloseHrmpChannelUnauthorized,
		/// The channel to be closed doesn't exist.
		CloseHrmpChannelDoesntExist,
		/// The channel close request is already requested.
		CloseHrmpChannelAlreadyUnderway,
	 }
}

decl_module! {
	/// The router module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;

		#[weight = 0]
		fn hrmp_init_open_channel(
			origin,
			recipient: ParaId,
			proposed_max_capacity: u32,
			proposed_max_message_size: u32,
		) -> DispatchResult {
			let origin = ensure_parachain(<T as Trait>::Origin::from(origin))?;
			Self::init_open_channel(
				origin,
				recipient,
				proposed_max_capacity,
				proposed_max_message_size
			)?;
			Ok(())
		}

		#[weight = 0]
		fn hrmp_accept_open_channel(origin, sender: ParaId) -> DispatchResult {
			let origin = ensure_parachain(<T as Trait>::Origin::from(origin))?;
			Self::accept_open_channel(origin, sender)?;
			Ok(())
		}

		#[weight = 0]
		fn hrmp_close_channel(origin, channel_id: HrmpChannelId) -> DispatchResult {
			let origin = ensure_parachain(<T as Trait>::Origin::from(origin))?;
			Self::close_channel(origin, channel_id)?;
			Ok(())
		}
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
		notification: &initializer::SessionChangeNotification<T::BlockNumber>,
	) {
		Self::perform_outgoing_para_cleanup();
		Self::process_hrmp_open_channel_requests(&notification.prev_config);
		Self::process_hrmp_close_channel_requests();
	}

	/// Iterate over all paras that were registered for offboarding and remove all the data
	/// associated with them.
	fn perform_outgoing_para_cleanup() {
		let outgoing = OutgoingParas::take();
		for outgoing_para in outgoing {
			Self::clean_dmp_after_outgoing(outgoing_para);
			Self::clean_ump_after_outgoing(outgoing_para);
			Self::clean_hrmp_after_outgoing(outgoing_para);
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
