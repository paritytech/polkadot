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

use crate::error::GetOnchainDisputesError;
use polkadot_node_subsystem::overseer;
use polkadot_primitives::v2::{CandidateHash, DisputeState, Hash, SessionIndex};
use std::collections::HashMap;

pub async fn get_onchain_disputes<Sender>(
	_sender: &mut Sender,
	_relay_parent: Hash,
) -> Result<HashMap<(SessionIndex, CandidateHash), DisputeState>, GetOnchainDisputesError>
where
	Sender: overseer::ProvisionerSenderTrait,
{
	let _onchain = Result::<
		HashMap<(SessionIndex, CandidateHash), DisputeState>,
		GetOnchainDisputesError,
	>::Ok(HashMap::new());
	#[cfg(feature = "staging-client")]
	let _onchain = self::staging_impl::get_onchain_disputes(_sender, _relay_parent).await;

	_onchain
}

// Merge this module with the outer (current one) when promoting to stable
#[cfg(feature = "staging-client")]
mod staging_impl {
	use super::*; // remove this when promoting to stable
	use crate::LOG_TARGET;
	use futures::channel::oneshot;
	use polkadot_node_subsystem::{
		errors::RuntimeApiError,
		messages::{RuntimeApiMessage, RuntimeApiRequest},
		SubsystemSender,
	};

	/// Gets the on-chain disputes at a given block number and returns them as a `HashSet` so that searching in them is cheap.
	pub async fn get_onchain_disputes<Sender>(
		sender: &mut Sender,
		relay_parent: Hash,
	) -> Result<HashMap<(SessionIndex, CandidateHash), DisputeState>, GetOnchainDisputesError> {
		gum::trace!(target: LOG_TARGET, ?relay_parent, "Fetching on-chain disputes");
		let (tx, rx) = oneshot::channel();
		sender
			.send_message(
				RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::StagingDisputes(tx))
					.into(),
			)
			.await;

		rx.await
			.map_err(|_| GetOnchainDisputesError::Channel)
			.and_then(|res| {
				res.map_err(|e| match e {
					RuntimeApiError::Execution { .. } =>
						GetOnchainDisputesError::Execution(e, relay_parent),
					RuntimeApiError::NotSupported { .. } =>
						GetOnchainDisputesError::NotSupported(e, relay_parent),
				})
			})
			.map(|v| v.into_iter().map(|e| ((e.0, e.1), e.2)).collect())
	}
}
