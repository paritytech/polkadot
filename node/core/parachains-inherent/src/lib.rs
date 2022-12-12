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

//! The parachain inherent data provider
//!
//! Parachain backing and approval is an off-chain process, but the parachain needs to progress on chain as well. To
//! make it progress on chain a block producer needs to forward information about the state of a parachain to the
//! runtime. This information is forwarded through an inherent to the runtime. Here we provide the
//! [`ParachainInherentDataProvider`] that requests the relevant data from the provisioner subsystem and creates the
//! the inherent data that the runtime will use to create an inherent.

#![deny(unused_crate_dependencies, unused_results)]

use futures::{select, FutureExt};
use polkadot_node_subsystem::{
	errors::SubsystemError, messages::ProvisionerMessage, overseer::Handle,
};
use polkadot_primitives::v2::{Block, Hash, InherentData as ParachainsInherentData};
use sp_runtime::generic::BlockId;
use std::{sync::Arc, time};

pub(crate) const LOG_TARGET: &str = "parachain::parachains-inherent";

/// How long to wait for the provisioner, before giving up.
const PROVISIONER_TIMEOUT: time::Duration = core::time::Duration::from_millis(2500);

/// Provides the parachains inherent data.
pub struct ParachainsInherentDataProvider<C: sp_blockchain::HeaderBackend<Block>> {
	pub client: Arc<C>,
	pub overseer: polkadot_overseer::Handle,
	pub parent: Hash,
}

impl<C: sp_blockchain::HeaderBackend<Block>> ParachainsInherentDataProvider<C> {
	/// Create a new [`Self`].
	pub fn new(client: Arc<C>, overseer: polkadot_overseer::Handle, parent: Hash) -> Self {
		ParachainsInherentDataProvider { client, overseer, parent }
	}

	/// Create a new instance of the [`ParachainsInherentDataProvider`].
	pub async fn create(
		client: Arc<C>,
		mut overseer: Handle,
		parent: Hash,
	) -> Result<ParachainsInherentData, Error> {
		let pid = async {
			let (sender, receiver) = futures::channel::oneshot::channel();
			gum::trace!(
				target: LOG_TARGET,
				relay_parent = ?parent,
				"Inherent data requested by Babe"
			);
			overseer.wait_for_activation(parent, sender).await;
			receiver
				.await
				.map_err(|_| Error::ClosedChannelAwaitingActivation)?
				.map_err(|e| Error::Subsystem(e))?;

			let (sender, receiver) = futures::channel::oneshot::channel();
			gum::trace!(
				target: LOG_TARGET,
				relay_parent = ?parent,
				"Requesting inherent data (after having waited for activation)"
			);
			overseer
				.send_msg(
					ProvisionerMessage::RequestInherentData(parent, sender),
					std::any::type_name::<Self>(),
				)
				.await;

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
				bitfields: pd.bitfields.into_iter().map(Into::into).collect(),
				backed_candidates: pd.backed_candidates,
				disputes: pd.disputes,
				parent_header,
			},
			Err(err) => {
				gum::debug!(
					target: LOG_TARGET,
					%err,
					"Could not get provisioner inherent data; injecting default data",
				);
				ParachainsInherentData {
					bitfields: Vec::new(),
					backed_candidates: Vec::new(),
					disputes: Vec::new(),
					parent_header,
				}
			},
		};

		Ok(inherent_data)
	}
}

#[async_trait::async_trait]
impl<C: sp_blockchain::HeaderBackend<Block>> sp_inherents::InherentDataProvider
	for ParachainsInherentDataProvider<C>
{
	async fn provide_inherent_data(
		&self,
		dst_inherent_data: &mut sp_inherents::InherentData,
	) -> Result<(), sp_inherents::Error> {
		let inherent_data = ParachainsInherentDataProvider::create(
			self.client.clone(),
			self.overseer.clone(),
			self.parent,
		)
		.await
		.map_err(|e| sp_inherents::Error::Application(Box::new(e)))?;

		dst_inherent_data
			.put_data(polkadot_primitives::v2::PARACHAINS_INHERENT_IDENTIFIER, &inherent_data)
	}

	async fn try_handle_error(
		&self,
		_identifier: &sp_inherents::InherentIdentifier,
		_error: &[u8],
	) -> Option<Result<(), sp_inherents::Error>> {
		// Inherent isn't checked and can not return any error
		None
	}
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("Blockchain error")]
	Blockchain(#[from] sp_blockchain::Error),
	#[error("Timeout: provisioner did not return inherent data after {:?}", PROVISIONER_TIMEOUT)]
	Timeout,
	#[error("Could not find the parent header in the blockchain: {:?}", _0)]
	ParentHeaderNotFound(Hash),
	#[error("Closed channel from overseer when awaiting activation")]
	ClosedChannelAwaitingActivation,
	#[error("Closed channel from provisioner when awaiting inherent data")]
	ClosedChannelAwaitingInherentData,
	#[error("Subsystem failed")]
	Subsystem(#[from] SubsystemError),
}
