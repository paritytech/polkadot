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

use futures::{future, Future, Stream, FutureExt, TryFutureExt, StreamExt, task::Spawn};
use log::warn;
use sc_client::BlockchainEvents;
use sp_core::Pair;
use sp_runtime::traits::BlakeTwo256;
use polkadot_primitives::{
	BlockId, Hash, Block,
	parachain::{
		self, BlockData, DutyRoster, HeadData, Id as ParaId,
		PoVBlock, ValidatorId, CollatorPair, LocalValidationData
	}
};
use polkadot_cli::{
	ProvideRuntimeApi, AbstractService, ParachainHost, IsKusama,
	service::{self, Roles}
};
pub use polkadot_cli::{VersionInfo, load_spec, service::Configuration};
pub use polkadot_validation::SignedStatement;
pub use polkadot_primitives::parachain::CollatorId;
pub use sc_network::PeerId;
pub use service::RuntimeApiCollection;

const COLLATION_TIMEOUT: Duration = Duration::from_secs(30);

/// An abstraction over the `Network` with useful functions for a `Collator`.
pub trait Network: Send + Sync {
	/// Create a `Stream` of checked statements for the given `relay_parent`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	fn checked_statements(&self, relay_parent: Hash) -> Pin<Box<dyn Stream<Item=SignedStatement>>>;
}

impl Network for polkadot_network::protocol::Service {
	fn checked_statements(&self, relay_parent: Hash) -> Pin<Box<dyn Stream<Item=SignedStatement>>> {
		polkadot_network::protocol::Service::checked_statements(self, relay_parent)
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
		network: impl Network + Clone + 'static,
	) -> Result<Self::ParachainContext, ()>
		where
			PolkadotClient<B, E, R>: ProvideRuntimeApi<Block>,
			<PolkadotClient<B, E, R> as ProvideRuntimeApi<Block>>::Api: RuntimeApiCollection<Extrinsic>,
			// Rust bug: https://github.com/rust-lang/rust/issues/24159
			<<PolkadotClient<B, E, R> as ProvideRuntimeApi<Block>>::Api as sp_api::ApiExt<Block>>::StateBackend:
				sp_api::StateBackend<BlakeTwo256>,
			Extrinsic: codec::Codec + Send + Sync + 'static,
			E: sc_client::CallExecutor<Block> + Clone + Send + Sync + 'static,
			SP: Spawn + Clone + Send + Sync + 'static,
			R: Send + Sync + 'static,
			B: sc_client_api::Backend<Block> + 'static,
			// Rust bug: https://github.com/rust-lang/rust/issues/24159
			B::State: sp_api::StateBackend<BlakeTwo256>;
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
		status: LocalValidationData,
	) -> Self::ProduceCandidate;
}

/// Produce a candidate for the parachain, with given contexts, parent head, and signing key.
pub async fn collate<P>(
	relay_parent: Hash,
	local_id: ParaId,
	local_validation_data: LocalValidationData,
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
		local_validation_data,
	).map_err(Error::Collator).await?;

	let pov_block = PoVBlock {
		block_data,
	};

	let pov_block_hash = pov_block.hash();
	let signature = key.sign(&parachain::collator_signature_payload(
		&relay_parent,
		&local_id,
		&pov_block_hash,
	));

	let info = parachain::CollationInfo {
		parachain_index: local_id,
		relay_parent,
		collator: key.public(),
		signature,
		head_data,
		pov_block_hash,
	};

	let collation = parachain::Collation {
		info,
		pov: pov_block,
	};

	Ok(collation)
}

fn build_collator_service<S, P, Extrinsic>(
	service: (S, polkadot_service::FullNodeHandles),
	para_id: ParaId,
	key: Arc<CollatorPair>,
	build_parachain_context: P,
) -> Result<S, polkadot_service::Error>
	where
		S: AbstractService<Block = service::Block>,
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
			sp_api::StateBackend<sp_runtime::traits::HashFor<Block>>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		S::CallExecutor: service::CallExecutor<service::Block>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		S::SelectChain: service::SelectChain<service::Block>,
		P: BuildParachainContext,
		P::ParachainContext: Send + 'static,
		<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
		Extrinsic: service::Codec + Send + Sync + 'static,
{
	let (service, handles) = service;
	let spawner = service.spawn_task_handle();

	let polkadot_network = match handles.polkadot_network {
		None => return Err(
			"Collator cannot run when Polkadot-specific networking has not been started".into()
		),
		Some(n) => n,
	};

	let client = service.client();

	let parachain_context = match build_parachain_context.build(
		client.clone(),
		spawner,
		polkadot_network.clone(),
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

			let network = polkadot_network.clone();
			let client = client.clone();
			let key = key.clone();
			let parachain_context = parachain_context.clone();

			let work = future::lazy(move |_| {
				let api = client.runtime_api();
				let local_validation = match try_fr!(api.local_validation_data(&id, para_id)) {
					Some(local_validation) => local_validation,
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
					local_validation,
					parachain_context,
					key,
				).map_ok(move |collation| {
					network.distribute_collation(targets, collation)
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

/// Async function that will run the collator node with the given `RelayChainContext` and `ParachainContext`
/// built by the given `BuildParachainContext` and arguments to the underlying polkadot node.
pub async fn start_collator<P>(
	build_parachain_context: P,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	config: Configuration,
) -> Result<(), polkadot_service::Error>
where
	P: BuildParachainContext,
	P::ParachainContext: Send + 'static,
	<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	let is_kusama = config.expect_chain_spec().is_kusama();
	match (is_kusama, config.roles) {
		(_, Roles::LIGHT) => return Err(
			polkadot_service::Error::Other("light nodes are unsupported as collator".into())
		).into(),
		(true, _) =>
			build_collator_service(
				service::kusama_new_full(config, Some((key.public(), para_id)), None, false, 6000, None)?,
				para_id,
				key,
				build_parachain_context,
			)?.await,
		(false, _) =>
			build_collator_service(
				service::polkadot_new_full(config, Some((key.public(), para_id)), None, false, 6000, None)?,
				para_id,
				key,
				build_parachain_context,
			)?.await,
	}
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
/// built by the given `BuildParachainContext` and arguments to the underlying polkadot node.
///
/// This function blocks until done.
pub fn run_collator<P>(
	build_parachain_context: P,
	para_id: ParaId,
	key: Arc<CollatorPair>,
	config: Configuration,
) -> polkadot_cli::Result<()> where
	P: BuildParachainContext,
	P::ParachainContext: Send + 'static,
	<P::ParachainContext as ParachainContext>::ProduceCandidate: Send,
{
	match (config.expect_chain_spec().is_kusama(), config.roles) {
		(_, Roles::LIGHT) => return Err(
			polkadot_cli::Error::Input("light nodes are unsupported as collator".into())
		).into(),
		(true, _) =>
			sc_cli::run_service_until_exit(config, |config| {
				build_collator_service(
					service::kusama_new_full(config, Some((key.public(), para_id)), None, false, 6000, None)?,
					para_id,
					key,
					build_parachain_context,
				)
			}),
		(false, _) =>
			sc_cli::run_service_until_exit(config, |config| {
				build_collator_service(
					service::polkadot_new_full(config, Some((key.public(), para_id)), None, false, 6000, None)?,
					para_id,
					key,
					build_parachain_context,
				)
			}),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Clone)]
	struct DummyParachainContext;

	impl ParachainContext for DummyParachainContext {
		type ProduceCandidate = future::Ready<Result<(BlockData, HeadData), InvalidHead>>;

		fn produce_candidate(
			&mut self,
			_relay_parent: Hash,
			_local_validation: LocalValidationData,
		) -> Self::ProduceCandidate {
			// send messages right back.
			future::ok((
				BlockData(vec![1, 2, 3, 4, 5,]),
				HeadData(vec![9, 9, 9]),
			))
		}
	}

	struct BuildDummyParachainContext;

	impl BuildParachainContext for BuildDummyParachainContext {
		type ParachainContext = DummyParachainContext;

		fn build<B, E, R, SP, Extrinsic>(
			self,
			_: Arc<PolkadotClient<B, E, R>>,
			_: SP,
			_: impl Network + Clone + 'static,
		) -> Result<Self::ParachainContext, ()> {
			Ok(DummyParachainContext)
		}
	}

	// Make sure that the future returned by `start_collator` implementes `Send`.
	#[test]
	fn start_collator_is_send() {
		fn check_send<T: Send>(_: T) {}

		check_send(start_collator(
			BuildDummyParachainContext,
			0.into(),
			Arc::new(CollatorPair::generate().0),
			Default::default(),
		));
	}
}
