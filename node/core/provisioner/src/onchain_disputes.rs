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

#![cfg(feature = "improved-onchain-disputes-import")]

use futures::channel::oneshot;
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{RuntimeApiMessage, RuntimeApiRequest},
	SubsystemSender,
};
use polkadot_primitives::v2::{CandidateHash, Hash, SessionIndex};
use std::collections::HashSet;

pub enum GetOnchainDisputesErr {
	Channel,
	Execution,
	NotSupported,
}

/// Gets the on-chain disputes at a given block number and returns them as a `HashSet` so that searching in them is cheap.
pub async fn get_onchain_disputes(
	sender: &mut impl SubsystemSender,
	relay_parent: Hash,
) -> Result<HashSet<(SessionIndex, CandidateHash)>, GetOnchainDisputesErr> {
	gum::trace!("Fetching on-chain disputes", ?relay_parent);
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(
			RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::Disputes(tx)).into(),
		)
		.await;

	rx.await
		.map_err(|_| {
			gum::error!("Channel error occured while fetching on-chain disputes", ?relay_parent);
			GetOnchainDisputesErr::Channel
		})
		.and_then(|res| {
			res.map_err(|e| match e {
				RuntimeApiError::Execution { .. } => {
					gum::error!(
						"Execution error occured while fetching on-chain disputes",
						?relay_parent
					);
					GetOnchainDisputesErr::Execution
				},
				RuntimeApiError::NotSupported { .. } => {
					gum::error!(
						"Runtime doesn't support on-chain disputes fetching",
						?relay_parent
					);
					GetOnchainDisputesErr::NotSupported
				},
			})
		})
		.map(|v| v.into_iter().map(|e| (e.0, e.1)).collect())
}
