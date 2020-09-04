// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Collation node logic.
//!
//! A collator node lives on a distinct parachain and submits a proposal for
//! a state transition, along with a proof for its validity
//! (what we might call a witness or block data).
//!
//! One of collators' other roles is to route messages between chains.
//! Each parachain produces a list of "egress" posts of messages for each other
//! parachain on each block, for a total of N^2 lists all together.
//!
//! We will refer to the egress list at relay chain block X of parachain A with
//! destination B as egress(X)[A -> B]
//!
//! On every block, each parachain will be intended to route messages from some
//! subset of all the other parachains. (NOTE: in practice this is not done until PoC-3)
//!
//! Since the egress information is unique to every block, when routing from a
//! parachain a collator must gather all egress posts from that parachain
//! up to the last point in history that messages were successfully routed
//! from that parachain, accounting for relay chain blocks where no candidate
//! from the collator's parachain was produced.
//!
//! In the case that all parachains route to each other and a candidate for the
//! collator's parachain was included in the last relay chain block, the collator
//! only has to gather egress posts from other parachains one block back in relay
//! chain history.
//!
//! This crate defines traits which provide context necessary for collation logic
//! to be performed, as the collation logic itself.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use std::pin::Pin;

use futures::{future, Future, Stream, FutureExt, StreamExt};
use sp_core::Pair;
use polkadot_primitives::v0::{
	BlockId, Hash, Block,
	BlockData, DutyRoster, HeadData, Id as ParaId,
	PoVBlock, ValidatorId, CollatorPair, LocalValidationData, GlobalValidationData,
	Collation, CollationInfo, collator_signature_payload,
};
use polkadot_cli::service::{self, Role};
pub use polkadot_cli::service::Configuration;
pub use polkadot_cli::Cli;
pub use polkadot_validation::SignedStatement;
pub use polkadot_primitives::v0::CollatorId;
pub use sc_network::PeerId;
pub use service::{RuntimeApiCollection, Client};
pub use sc_cli::SubstrateCli;
#[cfg(not(feature = "service-rewr"))]
use polkadot_service::{FullNodeHandles, AbstractClient};
#[cfg(feature = "service-rewr")]
use polkadot_service_new::{
	self as polkadot_service,
	Error as ServiceError, FullNodeHandles, AbstractClient,
};
use sc_service::SpawnTaskHandle;
use sp_core::traits::SpawnNamed;
use sp_runtime::traits::BlakeTwo256;
use consensus_common::SyncOracle;

const COLLATION_TIMEOUT: Duration = Duration::from_secs(30);

/// An abstraction over the `Network` with useful functions for a `Collator`.
pub trait Network: Send + Sync {
	/// Create a `Stream` of checked statements for the given `relay_parent`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	fn checked_statements(&self, relay_parent: Hash) -> Pin<Box<dyn Stream<Item=SignedStatement> + Send>>;
}

impl Network for polkadot_network::protocol::Service {
	fn checked_statements(&self, relay_parent: Hash) -> Pin<Box<dyn Stream<Item=SignedStatement> + Send>> {
		polkadot_network::protocol::Service::checked_statements(self, relay_parent).boxed()
	}
}

/// Collation errors.
#[derive(Debug)]
pub enum Error {
	/// Error on the relay-chain side of things.
	Polkadot(String),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			Error::Polkadot(ref err) => write!(f, "Polkadot node error: {}", err),
		}
	}
}

/// Something that can build a `ParachainContext`.
pub trait BuildParachainContext {
	/// The parachain context produced by the `build` function.
	type ParachainContext: self::ParachainContext;

	/// Build the `ParachainContext`.
	fn build<SP>(
		self,
		client: polkadot_service::Client,
		spawner: SP,
		network: impl Network + SyncOracle + Clone + 'static,
	) -> Result<Self::ParachainContext, ()>
		where
			SP: SpawnNamed + Clone + Send + Sync + 'static;
}

/// Parachain context needed for collation.
///
/// This can be implemented through an externally attached service or a stub.
/// This is expected to be a lightweight, shared type like an Arc.
pub trait ParachainContext: Clone {
	type ProduceCandidate: Future<Output = Option<(BlockData, HeadData)>>;

	/// Produce a candidate, given the relay parent hash, the latest ingress queue information
	/// and the last parachain head.
	fn produce_candidate(
		&mut self,
		relay_parent: Hash,
		global_validation: GlobalValidationData,
		local_validation: LocalValidationData,
		downward_messages: Vec<Vec<u8>>,
	) -> Self::ProduceCandidate;
}

/// Produce a candidate for the parachain, with given contexts, parent head, and signing key.
pub async fn collate<P>(
	relay_parent: Hash,
	local_id: ParaId,
	global_validation: GlobalValidationData,
	local_validation_data: LocalValidationData,
	downward_messages: Vec<Vec<u8>>,
	mut para_context: P,
	key: Arc<CollatorPair>,
) -> Option<Collation>
	where
		P: ParachainContext,
		P::ProduceCandidate: Send,
{
	let (block_data, head_data) = para_context.produce_candidate(
		relay_parent,
		global_validation,
		local_validation_data,
		downward_messages,
	).await?;

	let pov_block = PoVBlock {
		block_data,
	};

	let pov_block_hash = pov_block.hash();
	let signature = key.sign(&collator_signature_payload(
		&relay_parent,
		&local_id,
		&pov_block_hash,
	));

	let info = CollationInfo {
		parachain_index: local_id,
		relay_parent,
		collator: key.public(),
		signature,
		head_data,
		pov_block_hash,
	};

	let collation = Collation {
		info,
		pov: pov_block,
	};

	Some(collation)
}

#[cfg(feature = "service-rewr")]
fn build_collator_service<P>(
	spawner: SpawnTaskHandle,
	handles: FullNodeHandles,
	client: polkadot_service::Client,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	build_parachain_context: P,
) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>, polkadot_service::Error>
	where
		P: BuildParachainContext,
		P::ParachainContext: Send + 'static,
		<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	Err("Collator is not functional with the new service yet".into())
}

struct BuildCollationWork<P> {
	handles: polkadot_service::FullNodeHandles,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	build_parachain_context: P,
	spawner: SpawnTaskHandle,
	client: polkadot_service::Client,
}

impl<P> polkadot_service::ExecuteWithClient for BuildCollationWork<P>
	where
		P: BuildParachainContext,
		P::ParachainContext: Send + 'static,
		<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	type Output = Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>, polkadot_service::Error>;

	fn execute_with_client<Client, Api, Backend>(self, client: Arc<Client>) -> Self::Output
		where<Api as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
		Backend: sc_client_api::Backend<Block>,
		Backend::State: sp_api::StateBackend<BlakeTwo256>,
		Api: RuntimeApiCollection<StateBackend = Backend::State>,
		Client: AbstractClient<Block, Backend, Api = Api> + 'static
	{
		let polkadot_network = self.handles
			.polkadot_network
			.ok_or_else(|| "Collator cannot run when Polkadot-specific networking has not been started")?;

		// We don't require this here, but we need to make sure that the validation service is started.
		// This service makes sure the collator is joining the correct gossip topics and receives the appropiate
		// messages.
		self.handles.validation_service_handle
			.ok_or_else(|| "Collator cannot run when validation networking has not been started")?;

		let parachain_context = match self.build_parachain_context.build(
			self.client,
			self.spawner.clone(),
			polkadot_network.clone(),
		) {
			Ok(ctx) => ctx,
			Err(()) => {
				return Err("Could not build the parachain context!".into())
			}
		};

		let key = self.key;
		let para_id = self.para_id;
		let spawner = self.spawner;

		let res = async move {
			let mut notification_stream = client.import_notification_stream();

			while let Some(notification) = notification_stream.next().await {
				macro_rules! try_fr {
					($e:expr) => {
						match $e {
							Ok(x) => x,
							Err(e) => return future::Either::Left(future::err(Error::Polkadot(
								format!("{:?}", e)
							))),
						}
					}
				}

				let relay_parent = notification.hash;
				let id = BlockId::hash(relay_parent);

				let network = polkadot_network.clone();
				let client = client.clone();
				let key = key.clone();
				let parachain_context = parachain_context.clone();

				let work = future::lazy(move |_| {
					let api = client.runtime_api();
					let global_validation = try_fr!(api.global_validation_data(&id));
					let local_validation = match try_fr!(api.local_validation_data(&id, para_id)) {
						Some(local_validation) => local_validation,
						None => return future::Either::Left(future::ok(())),
					};
					let downward_messages = try_fr!(api.downward_messages(&id, para_id));

					let validators = try_fr!(api.validators(&id));

					let targets = compute_targets(
						para_id,
						validators.as_slice(),
						try_fr!(api.duty_roster(&id)),
					);

					let collation_work = collate(
						relay_parent,
						para_id,
						global_validation,
						local_validation,
						downward_messages,
						parachain_context,
						key,
					).map(move |collation| {
						match collation {
							Some(collation) => network.distribute_collation(targets, collation),
							None => log::trace!("Skipping collation as `collate` returned `None`"),
						}

						Ok(())
					});

					future::Either::Right(collation_work)
				});

				let deadlined = future::select(
					work.then(|f| f).boxed(),
					futures_timer::Delay::new(COLLATION_TIMEOUT)
				);

				let silenced = deadlined
					.map(|either| {
						match either {
							future::Either::Right(_) => log::warn!("Collation failure: timeout"),
							future::Either::Left((Err(e), _)) => {
								log::error!("Collation failed: {:?}", e)
							}
							future::Either::Left((Ok(()), _)) => {},
						}
					});

				let future = silenced.map(drop);

				spawner.spawn("collation-work", future);
			}
		};

		Ok(res.boxed())
	}
}

#[cfg(not(feature = "service-rewr"))]
fn build_collator_service<P>(
	spawner: SpawnTaskHandle,
	handles: FullNodeHandles,
	client: polkadot_service::Client,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	build_parachain_context: P,
) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>, polkadot_service::Error>
	where
		P: BuildParachainContext,
		P::ParachainContext: Send + 'static,
		<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	client.execute_with(BuildCollationWork {
		handles,
		para_id,
		key,
		build_parachain_context,
		spawner,
		client: client.clone(),
	})
}

/// Async function that will run the collator node with the given `RelayChainContext` and `ParachainContext`
/// built by the given `BuildParachainContext` and arguments to the underlying polkadot node.
pub fn start_collator<P>(
	build_parachain_context: P,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	config: Configuration,
) -> Result<
	(Pin<Box<dyn Future<Output = ()> + Send>>, sc_service::TaskManager),
	polkadot_service::Error
>
where
	P: 'static + BuildParachainContext,
	P::ParachainContext: Send + 'static,
	<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	if matches!(config.role, Role::Light) {
		return Err(
			polkadot_service::Error::Other("light nodes are unsupported as collator".into())
		.into());
	}

	let (task_manager, client, handles) = polkadot_service::build_full(
		config,
		Some((key.public(), para_id)),
		None,
		false,
		6000,
		None,
	)?;

	let future = build_collator_service(
		task_manager.spawn_handle(),
		handles,
		client,
		para_id,
		key,
		build_parachain_context,
	)?;

	Ok((future, task_manager))
}

#[cfg(not(feature = "service-rewr"))]
fn compute_targets(para_id: ParaId, session_keys: &[ValidatorId], roster: DutyRoster) -> HashSet<ValidatorId> {
	use polkadot_primitives::v0::Chain;

	roster.validator_duty.iter().enumerate()
		.filter(|&(_, c)| c == &Chain::Parachain(para_id))
		.filter_map(|(i, _)| session_keys.get(i))
		.cloned()
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Clone)]
	struct DummyParachainContext;

	impl ParachainContext for DummyParachainContext {
		type ProduceCandidate = future::Ready<Option<(BlockData, HeadData)>>;

		fn produce_candidate(
			&mut self,
			_relay_parent: Hash,
			_global: GlobalValidationData,
			_local_validation: LocalValidationData,
			_: Vec<Vec<u8>>,
		) -> Self::ProduceCandidate {
			// send messages right back.
			future::ready(Some((
				BlockData(vec![1, 2, 3, 4, 5,]),
				HeadData(vec![9, 9, 9]),
			)))
		}
	}

	struct BuildDummyParachainContext;

	impl BuildParachainContext for BuildDummyParachainContext {
		type ParachainContext = DummyParachainContext;

		fn build<SP>(
			self,
			_: polkadot_service::Client,
			_: SP,
			_: impl Network + Clone + 'static,
		) -> Result<Self::ParachainContext, ()> {
			Ok(DummyParachainContext)
		}
	}

	// Make sure that the future returned by `start_collator` implements `Send`.
	#[test]
	fn start_collator_is_send() {
		fn check_send<T: Send>(_: T) {}

		let cli = Cli::from_iter(&["-dev"]);
		let task_executor = |_, _| async {};
		let config = cli.create_configuration(&cli.run.base, task_executor.into()).unwrap();

		check_send(start_collator(
			BuildDummyParachainContext,
			0.into(),
			Arc::new(CollatorPair::generate().0),
			config,
		));
	}
}
