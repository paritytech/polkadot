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

//! The proposer proposes new blocks to include

#![deny(unused_crate_dependencies, unused_results)]

use futures::{select, FutureExt};
use polkadot_node_subsystem::{
	messages::{AllMessages, ProvisionerMessage}, SubsystemError,
};
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::v1::{
	Block, Hash, InherentData as ParachainsInherentData,
};
use sp_blockchain::HeaderBackend;
use sp_runtime::generic::BlockId;
use std::time;

/// How long to wait for the provisition, before giving up.
const PROVISIONER_TIMEOUT: time::Duration = core::time::Duration::from_millis(2500);

pub struct ParachainsInherentDataProvider {
	inherent_data: ParachainsInherentData,
}

impl ParachainsInherentDataProvider {
	pub async fn create<C: HeaderBackend<Block>>(
		client: &C,
		mut overseer: OverseerHandler,
		parent: Hash,
	) -> Result<Self, Error> {
		let pid = async {
			let (sender, receiver) = futures::channel::oneshot::channel();
			overseer.wait_for_activation(parent, sender).await;
			receiver.await.map_err(|_| Error::ClosedChannelAwaitingActivation)?.map_err(Error::Subsystem)?;

			let (sender, receiver) = futures::channel::oneshot::channel();
			overseer.send_msg(AllMessages::Provisioner(
				ProvisionerMessage::RequestInherentData(parent, sender),
			)).await;

			receiver.await.map_err(|_| Error::ClosedChannelAwaitingInherentData)
		};

		let mut timeout = futures_timer::Delay::new(PROVISIONER_TIMEOUT).fuse();

		let parent_header = match client.header(BlockId::Hash(parent)) {
			Ok(Some(h)) => h,
			Ok(None) => return Err(Error::ParentHeaderNotFound(parent)),
			Err(err) => return Err(Error::Blockchain(err)),
		};

		let res = select! {
			pid = pid.fuse() => pid,
			_ = timeout => Err(Error::Timeout),
		};

		let inherent_data = match res {
			Ok(pd) => ParachainsInherentData {
				bitfields: pd.bitfields,
				backed_candidates: pd.backed_candidates,
				disputes: pd.disputes,
				parent_header,
			},
			Err(err) => {
				tracing::debug!(
					?err,
					"Could not get provisioner inherent data; injecting default data",
				);
				ParachainsInherentData {
					bitfields: Vec::new(),
					backed_candidates: Vec::new(),
					disputes: Vec::new(),
					parent_header,
				}
			}
		};

		Ok(Self { inherent_data })
	}
}

#[async_trait::async_trait]
impl sp_inherents::InherentDataProvider for ParachainsInherentDataProvider {
	fn provide_inherent_data(&self, inherent_data: &mut sp_inherents::InherentData) -> Result<(), sp_inherents::Error> {
		inherent_data.put_data(
			polkadot_primitives::v1::PARACHAINS_INHERENT_IDENTIFIER,
			&self.inherent_data,
		)
	}

	async fn try_handle_error(
		&self,
		_: &sp_inherents::InherentIdentifier,
		_: &[u8],
	) -> Option<Result<(), sp_inherents::Error>> {
		// Inherent isn't checked and can not return any error
		None
	}
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("Blockchain error")]
	Blockchain(sp_blockchain::Error),
	#[error("Timeout: provisioner did not return inherent data after {:?}", PROVISIONER_TIMEOUT)]
	Timeout,
	#[error("Could not find the parent header in the blockchain: {:?}", _0)]
	ParentHeaderNotFound(Hash),
	#[error("Closed channel from overseer when awaiting activation")]
	ClosedChannelAwaitingActivation,
	#[error("Closed channel from provisioner when awaiting inherent data")]
	ClosedChannelAwaitingInherentData,
	#[error("Subsystem failed")]
	Subsystem(SubsystemError),
}
