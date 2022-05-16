// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

//! The disputes module is responsible for selecting dispute votes to be sent with the inherent data. It contains two
//! different implementations, extracted in two separate modules - `with_staging_api` and `without_staging_api`. The
//! active one is controlled with a feature flag (`staging-client`). The entrypoint to these implementations is the
//! `select_disputes` function. Refer to the documentation of the modules for more details about each implementation.

use crate::LOG_TARGET;
use futures::channel::oneshot;
use polkadot_node_primitives::CandidateVotes;
use polkadot_node_subsystem::{messages::DisputeCoordinatorMessage, overseer};
use polkadot_primitives::v2::{CandidateHash, SessionIndex};

/// Request the relevant dispute statements for a set of disputes identified by `CandidateHash` and the `SessionIndex`.
async fn request_votes(
	sender: &mut impl overseer::ProvisionerSenderTrait,
	disputes_to_query: Vec<(SessionIndex, CandidateHash)>,
) -> Vec<(SessionIndex, CandidateHash, CandidateVotes)> {
	let (tx, rx) = oneshot::channel();
	// Bounded by block production - `ProvisionerMessage::RequestInherentData`.
	sender.send_unbounded_message(DisputeCoordinatorMessage::QueryCandidateVotes(
		disputes_to_query,
		tx,
	));

	match rx.await {
		Ok(v) => v,
		Err(oneshot::Canceled) => {
			gum::warn!(target: LOG_TARGET, "Unable to query candidate votes");
			Vec::new()
		},
	}
}

#[cfg(feature = "staging-client")]
pub(crate) mod with_staging_api;

#[cfg(not(feature = "staging-client"))]
pub(crate) mod without_staging_api;
