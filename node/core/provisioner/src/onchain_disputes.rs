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

#![cfg(feature = "staging-client")]

use crate::LOG_TARGET;
use futures::channel::oneshot;
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{RuntimeApiMessage, RuntimeApiRequest},
	SubsystemSender,
};
use polkadot_primitives::v2::{CandidateHash, DisputeState, Hash, SessionIndex};
use std::collections::HashMap;

pub enum GetOnchainDisputesErr {
	Channel,
	Execution,
	NotSupported,
}

/// Gets the on-chain disputes at a given block number and returns them as a `HashSet` so that searching in them is cheap.
pub async fn get_onchain_disputes(
	sender: &mut impl SubsystemSender,
	relay_parent: Hash,
) -> Result<HashMap<(SessionIndex, CandidateHash), DisputeState>, GetOnchainDisputesErr> {
	gum::trace!(target: LOG_TARGET, ?relay_parent, "Fetching on-chain disputes");
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(
			RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::StagingDisputes(tx)).into(),
		)
		.await;

	rx.await
		.map_err(|_| {
			gum::error!(
				target: LOG_TARGET,
				?relay_parent,
				"Channel error occurred while fetching on-chain disputes"
			);
			GetOnchainDisputesErr::Channel
		})
		.and_then(|res| {
			res.map_err(|e| match e {
				RuntimeApiError::Execution { .. } => {
					gum::error!(
						target: LOG_TARGET,
						?relay_parent,
						"Execution error occurred while fetching on-chain disputes",
					);
					GetOnchainDisputesErr::Execution
				},
				RuntimeApiError::NotSupported { .. } => {
					gum::error!(
						target: LOG_TARGET,
						?relay_parent,
						"Runtime doesn't support on-chain disputes fetching",
					);
					GetOnchainDisputesErr::NotSupported
				},
			})
		})
		.map(|v| v.into_iter().map(|e| ((e.0, e.1), e.2)).collect())
}
