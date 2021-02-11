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

use std::rc::Rc;
use std::collections::HashSet;

use futures::channel::oneshot;

use polkadot_primitives::v1::{
	BlakeTwo256, CoreState, ErasureChunk, Hash, HashT,
	SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID, CandidateHash,
	CandidateDescriptor, OccupiedCore,
};
use polkadot_subsystem::{
	jaeger, errors::{ChainApiError, RuntimeApiError}, PerLeafSpan,
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemError,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, ChainApiMessage,
	NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest, NetworkBridgeEvent
};
use super::{session_cache::SessionInfo, LOG_TARGET};

pub struct FetchTask {
	/// For what relay parents this task is relevant.
	///
	/// In other words, for which relay chain parents this candidate is considered live.
	/// This is updated on every `ActiveLeavesUpdate` and enables us to know when we can safely
	/// stop keeping track of that candidate/chunk.
	live_in: HashSet<Hash>,

	/// The relay parent providing the context for the candidate.
	relay_parent: Hash,

	/// Some details about the to be fetched candidate.
	descriptor: CandidateDescriptor,

	/// We keep the task around in state `Fetched` until `live_in` becomes empty, to make
	/// sure we won't re-fetch an already fetched candidate.
	state: FetchedState,

    session: Rc<SessionInfo>
}

/// State of a particular candidate chunk fetching process.
enum FetchedState {
	/// Chunk is currently being fetched.
	Fetching,
	/// Chunk has already been fetched successfully.
	Fetched,
	/// All relevant live_in have been removed, before we were able to get our chunk.
	Canceled,
}

impl FetchTask {
//	/// Start fetching a chunk.
	// pub async fn start<Context>(ctx: &mut Context, leaf: Hash, core: OccupiedCore) -> Self
    // where
    //     Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
    // {
	//     panic
	// }

	/// Add the given leaf to the relay parents which are making this task relevant.
	pub fn add_leaf(&mut self, leaf: Hash) {
		self.live_in.insert(leaf);
	}

	/// Remove leaves and cancel the task, if it was the last one and the task has still been
	/// fetching.
	pub fn remove_leaves(&mut self, leaves: HashSet<Hash>) {
		self.live_in.difference(leaves);
		if self.live_in.is_empty() {
			// TODO: Make sure, to actually cancel the task.
			self.state = FetchedState::Canceled
		}
	}

	/// Whether or not this task can be considered finished.
	///
	/// That is, it is either canceled or succeeded fetching the chunk.
	pub fn is_finished(&self) -> bool {
		match self.state {
			FetchedState::Fetched | FetchedState::Canceled => true,
			FetchedState::Fetching => false,
		}
	}

	/// Retrieve the relay parent providing the context for this candidate.
	pub fn get_relay_parent(&self) -> Hash {
		self.relay_parent
	}
}
