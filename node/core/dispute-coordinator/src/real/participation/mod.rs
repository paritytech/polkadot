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
use polkadot_primitives::v1::CandidateHash;
use queues::Queues;

/// Keep track of disputes we need to participate in.
///
/// - Prioritize and queue participations
/// - Dequeue participation requests in order and send them to the participation subsystem.
struct Participation {
	/// Participations currently being processed.
	running_participations: HashSet<CandidateHash>,
	queue: Queues,
}
