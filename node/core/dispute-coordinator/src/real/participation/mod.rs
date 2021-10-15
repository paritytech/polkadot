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

use std::collections::HashSet;

mod queues;
use futures::channel::mpsc;
use polkadot_node_subsystem::SubsystemContext;
use polkadot_primitives::v1::CandidateHash;
use queues::Queues;

/// Keep track of disputes we need to participate in.
///
/// - Prioritize and queue participations
/// - Dequeue participation requests in order and launch participation worker.
struct Participation {
	/// Participations currently being processed.
	running_participations: HashSet<CandidateHash>,
	/// Priority and best effort queues.
	queue: Queues,
	/// Sender to be passed to worker tasks.
	worker_sender: mpsc::Sender<WorkerMessage>,
}

enum WorkerMessage {
}

impl Participation {
	/// Get ready for managing dispute participation requests.
	///
	/// The passed in sender will be used by background workers to communicate back their results.
	/// The calling context should make sure to call `Participation::on_worker_message()` for the
	/// received messages.
	pub fn new(sender: mpsc::Sender<WorkerMessage>) -> Self {}

	/// Queue a dispute for the node to participate in:
	async fn queue_participation<Context: SubsystemContext>(&mut self, ctx: &mut Context, ordering: &mut OrderingProvider, candidate: CandidateReceipt, session: SessionIndex)  {}

	async fn on_worker_message(&mut self, ctx: &mut Context, msg: WorkerMessage) {}
}


