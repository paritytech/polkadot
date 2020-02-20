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

use futures::{future, Future, Stream, FutureExt, TryFutureExt, StreamExt, task::Spawn};
use log::warn;
use sc_client::BlockchainEvents;
use sp_core::{Pair, Blake2Hasher};
use polkadot_primitives::{
	BlockId, Hash, Block,
	parachain::{
		self, BlockData, DutyRoster, HeadData, Id as ParaId,
		PoVBlock, Status as ParachainStatus, ValidatorId, CollatorPair,
	}
};
use polkadot_cli::{
	ProvideRuntimeApi, AbstractService, ParachainHost, IsKusama,
	service::{self, Roles, SelectChain}
};
use polkadot_network::legacy::validation::ValidationNetwork;

pub use polkadot_cli::{VersionInfo, load_spec, service::Configuration};
pub use polkadot_validation::SignedStatement;
pub use polkadot_primitives::parachain::CollatorId;
pub use sc_network::PeerId;
pub use service::RuntimeApiCollection;

const COLLATION_TIMEOUT: Duration = Duration::from_secs(30);

/// An abstraction over the `Network` with useful functions for a `Collator`.
pub trait Network: Send + Sync {
	/// Convert the given `CollatorId` to a `PeerId`.
	fn collator_id_to_peer_id(&self, collator_id: CollatorId) ->
		Box<dyn Future<Output=Option<PeerId>> + Send>;

	/// Create a `Stream` of checked statements for the given `relay_parent`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	fn checked_statements(&self, relay_parent: Hash) -> Box<dyn Stream<Item=SignedStatement>>;
}

impl<P, SP> Network for ValidationNetwork<P, SP> where
	P: 'static + Send + Sync,
	SP: 'static + Spawn + Clone + Send + Sync,
{
	fn collator_id_to_peer_id(&self, collator_id: CollatorId) ->
		Box<dyn Future<Output=Option<PeerId>> + Send>
	{
		Box::new(Self::collator_id_to_peer_id(self, collator_id))
	}

	fn checked_statements(&self, relay_parent: Hash) -> Box<dyn Stream<Item=SignedStatement>> {
		Box::new(Self::checked_statements(self, relay_parent))
	}
}

/// Error to return when the head data was invalid.
#[derive(Clone, Copy, Debug)]
pub struct InvalidHead;

/// Collation errors.
#[derive(Debug)]
pub enum Error {
	/// Error on the relay-chain side of things.
	Polkadot(String),
	/// Error on the collator side of things.
	Collator(InvalidHead),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			Error::Polkadot(ref err) => write!(f, "Polkadot node error: {}", err),
			Error::Collator(_) => write!(f, "Collator node error: Invalid head data"),
		}
	}
}

/// The Polkadot client type.
pub type PolkadotClient<B, E, R> = sc_client::Client<B, E, Block, R>;

/// Something that can build a `ParachainContext`.
pub trait BuildParachainContext {
	/// The parachain context produced by the `build` function.
	type ParachainContext: self::ParachainContext;

	/// Build the `ParachainContext`.
	fn build<B, E, R, SP, Extrinsic>(
		self,
		client: Arc<PolkadotClient<B, E, R>>,
		spawner: SP,
		network: Arc<dyn Network>,
	) -> Result<Self::ParachainContext, ()>
		where
			PolkadotClient<B, E, R>: ProvideRuntimeApi<Block>,
			<PolkadotClient<B, E, R> as ProvideRuntimeApi<Block>>::Api: RuntimeApiCollection<Extrinsic>,
			// Rust bug: https://github.com/rust-lang/rust/issues/24159
			<<PolkadotClient<B, E, R> as ProvideRuntimeApi<Block>>::Api as sp_api::ApiExt<Block>>::StateBackend:
				sp_api::StateBackend<Blake2Hasher>,
			Extrinsic: codec::Codec + Send + Sync + 'static,
			E: sc_client::CallExecutor<Block> + Clone + Send + Sync + 'static,
			SP: Spawn + Clone + Send + Sync + 'static,
			R: Send + Sync + 'static,
			B: sc_client_api::Backend<Block> + 'static,
			// Rust bug: https://github.com/rust-lang/rust/issues/24159
			B::State: sp_api::StateBackend<Blake2Hasher>;
}

/// Parachain context needed for collation.
///
/// This can be implemented through an externally attached service or a stub.
/// This is expected to be a lightweight, shared type like an Arc.
pub trait ParachainContext: Clone {
	type ProduceCandidate: Future<Output = Result<(BlockData, HeadData), InvalidHead>>;

	/// Produce a candidate, given the relay parent hash, the latest ingress queue information
	/// and the last parachain head.
	fn produce_candidate(
		&mut self,
		relay_parent: Hash,
		status: ParachainStatus,
	) -> Self::ProduceCandidate;
}

/// Produce a candidate for the parachain, with given contexts, parent head, and signing key.
pub async fn collate<P>(
	relay_parent: Hash,
	local_id: ParaId,
	parachain_status: ParachainStatus,
	mut para_context: P,
	key: Arc<CollatorPair>,
)
	-> Result<parachain::Collation, Error>
	where
		P: ParachainContext,
		P::ProduceCandidate: Send,
{
	let (block_data, head_data) = para_context.produce_candidate(
		relay_parent,
		parachain_status,
	).map_err(Error::Collator).await?;

	let block_data_hash = block_data.hash();
	let signature = key.sign(block_data_hash.as_ref());
	let info = parachain::CollationInfo {
		parachain_index: local_id,
		collator: key.public(),
		signature,
		head_data,
		block_data_hash,
		upward_messages: Vec::new(),
	};

	let collation = parachain::Collation {
		info,
		pov: PoVBlock {
			block_data,
		},
	};

	Ok(collation)
}

/// Run the collator node using the given `service`.
fn run_collator_node<S, P, Extrinsic>(
	service: S,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	build_parachain_context: P,
) -> Result<S, polkadot_service::Error>
	where
		S: AbstractService<Block = service::Block, NetworkSpecialization = service::PolkadotProtocol>,
		sc_client::Client<S::Backend, S::CallExecutor, service::Block, S::RuntimeApi>: ProvideRuntimeApi<Block>,
		<sc_client::Client<S::Backend, S::CallExecutor, service::Block, S::RuntimeApi> as ProvideRuntimeApi<Block>>::Api:
			RuntimeApiCollection<
				Extrinsic,
				Error = sp_blockchain::Error,
				StateBackend = sc_client_api::StateBackendFor<S::Backend, Block>
			>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		S::Backend: service::Backend<service::Block>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		<S::Backend as service::Backend<service::Block>>::State:
			sp_api::StateBackend<sp_runtime::traits::HasherFor<Block>>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		S::CallExecutor: service::CallExecutor<service::Block>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		S::SelectChain: service::SelectChain<service::Block>,
		P: BuildParachainContext,
		P::ParachainContext: Send + 'static,
		<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
		Extrinsic: service::Codec + Send + Sync + 'static,
{
	let spawner = service.spawn_task_handle();

	let client = service.client();
	let network = service.network();
	let known_oracle = client.clone();
	let select_chain = if let Some(select_chain) = service.select_chain() {
		select_chain
	} else {
		return Err("The node cannot work because it can't select chain.".into())
	};

	let is_known = move |block_hash: &Hash| {
		use consensus_common::BlockStatus;
		use polkadot_network::legacy::gossip::Known;

		match known_oracle.block_status(&BlockId::hash(*block_hash)) {
			Err(_) | Ok(BlockStatus::Unknown) | Ok(BlockStatus::Queued) => None,
			Ok(BlockStatus::KnownBad) => Some(Known::Bad),
			Ok(BlockStatus::InChainWithState) | Ok(BlockStatus::InChainPruned) =>
				match select_chain.leaves() {
					Err(_) => None,
					Ok(leaves) => if leaves.contains(block_hash) {
						Some(Known::Leaf)
					} else {
						Some(Known::Old)
					},
				}
		}
	};

	let message_validator = polkadot_network::legacy::gossip::register_validator(
		network.clone(),
		(is_known, client.clone()),
		&spawner,
	);

	let validation_network = Arc::new(ValidationNetwork::new(
		message_validator,
		client.clone(),
		spawner.clone(),
	));

	let parachain_context = match build_parachain_context.build(
		client.clone(),
		spawner,
		validation_network.clone(),
	) {
		Ok(ctx) => ctx,
		Err(()) => {
			return Err("Could not build the parachain context!".into())
		}
	};

	let work = async move {
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

			let network = network.clone();
			let client = client.clone();
			let key = key.clone();
			let parachain_context = parachain_context.clone();

			let work = future::lazy(move |_| {
				let api = client.runtime_api();
				let status = match try_fr!(api.parachain_status(&id, para_id)) {
					Some(status) => status,
					None => return future::Either::Left(future::ok(())),
				};

				let validators = try_fr!(api.validators(&id));

				let targets = compute_targets(
					para_id,
					validators.as_slice(),
					try_fr!(api.duty_roster(&id)),
				);

				let collation_work = collate(
					relay_parent,
					para_id,
					status,
					parachain_context,
					key,
				).map_ok(move |collation| {
					network.with_spec(move |spec, ctx| {
						let res = spec.add_local_collation(
							ctx,
							relay_parent,
							targets,
							collation,
						);

						tokio::spawn(res.boxed());
					})
				});

				future::Either::Right(collation_work)
			});

			let deadlined = future::select(
				work.then(|f| f).boxed(),
				futures_timer::Delay::new(COLLATION_TIMEOUT)
			);

			let silenced = deadlined
				.map(|either| {
					if let future::Either::Right(_) = either {
						warn!("Collation failure: timeout");
					}
				});

			let future = silenced.map(drop);

			tokio::spawn(future);
		}
	}.boxed();

	service.spawn_essential_task("collation", work);

	Ok(service)
}

fn compute_targets(para_id: ParaId, session_keys: &[ValidatorId], roster: DutyRoster) -> HashSet<ValidatorId> {
	use polkadot_primitives::parachain::Chain;

	roster.validator_duty.iter().enumerate()
		.filter(|&(_, c)| c == &Chain::Parachain(para_id))
		.filter_map(|(i, _)| session_keys.get(i))
		.cloned()
		.collect()
}

/// Run a collator node with the given `RelayChainContext` and `ParachainContext`
/// build by the given `BuildParachainContext` and arguments to the underlying polkadot node.
///
/// This function blocks until done.
pub fn run_collator<P>(
	build_parachain_context: P,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	config: Configuration,
) -> polkadot_cli::error::Result<()> where
	P: BuildParachainContext,
	P::ParachainContext: Send + 'static,
	<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	match (config.expect_chain_spec().is_kusama(), config.roles) {
		(true, Roles::LIGHT) =>
			sc_cli::run_service_until_exit(config, |config| {
				run_collator_node(
					service::kusama_new_light(config, Some((key.public(), para_id)))?,
					para_id,
					key,
					build_parachain_context,
				)
			}),
		(true, _) =>
			sc_cli::run_service_until_exit(config, |config| {
				run_collator_node(
					service::kusama_new_full(config, Some((key.public(), para_id)), None, false, 6000)?,
					para_id,
					key,
					build_parachain_context,
				)
			}),
		(false, Roles::LIGHT) =>
			sc_cli::run_service_until_exit(config, |config| {
				run_collator_node(
					service::polkadot_new_light(config, Some((key.public(), para_id)))?,
					para_id,
					key,
					build_parachain_context,
				)
			}),
		(false, _) =>
			sc_cli::run_service_until_exit(config, |config| {
				run_collator_node(
					service::polkadot_new_full(config, Some((key.public(), para_id)), None, false, 6000)?,
					para_id,
					key,
					build_parachain_context,
				)
			}),
	}
}

#[cfg(test)]
mod tests {
	use polkadot_primitives::parachain::FeeSchedule;
	use keyring::Sr25519Keyring;
	use super::*;

	#[derive(Clone)]
	struct DummyParachainContext;

	impl ParachainContext for DummyParachainContext {
		type ProduceCandidate = future::Ready<Result<(BlockData, HeadData), InvalidHead>>;

		fn produce_candidate(
			&mut self,
			_relay_parent: Hash,
			_status: ParachainStatus,
		) -> Self::ProduceCandidate {
			// send messages right back.
			future::ok((
				BlockData(vec![1, 2, 3, 4, 5,]),
				HeadData(vec![9, 9, 9]),
			))
		}
	}
}
