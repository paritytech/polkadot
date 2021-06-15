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

//! # Overseer
//!
//! `overseer` implements the Overseer architecture described in the
//! [implementers-guide](https://w3f.github.io/parachain-implementers-guide/node/index.html).
//! For the motivations behind implementing the overseer itself you should
//! check out that guide, documentation in this crate will be mostly discussing
//! technical stuff.
//!
//! An `Overseer` is something that allows spawning/stopping and overseing
//! asynchronous tasks as well as establishing a well-defined and easy to use
//! protocol that the tasks can use to communicate with each other. It is desired
//! that this protocol is the only way tasks communicate with each other, however
//! at this moment there are no foolproof guards against other ways of communication.
//!
//! The `Overseer` is instantiated with a pre-defined set of `Subsystems` that
//! share the same behavior from `Overseer`'s point of view.
//!
//! ```text
//!                              +-----------------------------+
//!                              |         Overseer            |
//!                              +-----------------------------+
//!
//!             ................|  Overseer "holds" these and uses |..............
//!             .                  them to (re)start things                      .
//!             .                                                                .
//!             .  +-------------------+                +---------------------+  .
//!             .  |   Subsystem1      |                |   Subsystem2        |  .
//!             .  +-------------------+                +---------------------+  .
//!             .           |                                       |            .
//!             ..................................................................
//!                         |                                       |
//!                       start()                                 start()
//!                         V                                       V
//!             ..................| Overseer "runs" these |.......................
//!             .  +--------------------+               +---------------------+  .
//!             .  | SubsystemInstance1 |               | SubsystemInstance2  |  .
//!             .  +--------------------+               +---------------------+  .
//!             ..................................................................
//! ```

// #![deny(unused_results)]
// unused dependencies can not work for test and examples at the same time
// yielding false positives
#![warn(missing_docs)]

use std::fmt::{self, Debug};
use std::pin::Pin;
use std::sync::{atomic::{self, AtomicUsize}, Arc};
use std::task::Poll;
use std::time::Duration;
use std::collections::{hash_map, HashMap};

use futures::channel::oneshot;
use futures::{
	poll, select,
	future::BoxFuture,
	stream::{self, FuturesUnordered, Fuse},
	Future, FutureExt, StreamExt,
};
use futures_timer::Delay;
use lru::LruCache;

use polkadot_primitives::v1::{Block, BlockId,BlockNumber, Hash, ParachainHost};
use client::{BlockImportNotification, BlockchainEvents, FinalityNotification};
use sp_api::{ApiExt, ProvideRuntimeApi};

use polkadot_subsystem::messages::{
	CandidateValidationMessage, CandidateBackingMessage,
	ChainApiMessage, StatementDistributionMessage,
	AvailabilityDistributionMessage, BitfieldSigningMessage, BitfieldDistributionMessage,
	ProvisionerMessage, RuntimeApiMessage,
	AvailabilityStoreMessage, NetworkBridgeMessage, AllMessages, CollationGenerationMessage,
	CollatorProtocolMessage, AvailabilityRecoveryMessage, ApprovalDistributionMessage,
	ApprovalVotingMessage, GossipSupportMessage,
};
pub use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemSender, OverseerSignal, FromOverseer, SubsystemError,
	SubsystemResult, SpawnedSubsystem, ActiveLeavesUpdate, ActivatedLeaf, DummySubsystem, jaeger,
	LeafStatus,
};
use polkadot_node_subsystem_util::{TimeoutExt, metrics::{self, prometheus}, metered, Metronome};
use polkadot_node_primitives::SpawnNamed;
use polkadot_procmacro_overseer_subsystems_gen::AllSubsystemsGen;

#[cfg(test)]
mod tests;

// A capacity of bounded channels inside the overseer.
const CHANNEL_CAPACITY: usize = 1024;
// The capacity of signal channels to subsystems.
const SIGNAL_CHANNEL_CAPACITY: usize = 64;

// A graceful `Overseer` teardown time delay.
const STOP_DELAY: u64 = 1;
// Target for logs.
const LOG_TARGET: &'static str = "parachain::overseer";

trait MapSubsystem<T> {
	type Output;

	fn map_subsystem(&self, sub: T) -> Self::Output;
}

impl<F, T, U> MapSubsystem<T> for F where F: Fn(T) -> U {
	type Output = U;

	fn map_subsystem(&self, sub: T) -> U {
		(self)(sub)
	}
}

/// Whether a header supports parachain consensus or not.
pub trait HeadSupportsParachains {
	/// Return true if the given header supports parachain consensus. Otherwise, false.
	fn head_supports_parachains(&self, head: &Hash) -> bool;
}

impl<Client> HeadSupportsParachains for Arc<Client> where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	fn head_supports_parachains(&self, head: &Hash) -> bool {
		let id = BlockId::Hash(*head);
		self.runtime_api().has_api::<dyn ParachainHost<Block>>(&id).unwrap_or(false)
	}
}

/// This struct is passed as an argument to create a new instance of an [`Overseer`].
///
/// As any entity that satisfies the interface may act as a [`Subsystem`] this allows
/// mocking in the test code:
///
/// Each [`Subsystem`] is supposed to implement some interface that is generic over
/// message type that is specific to this [`Subsystem`]. At the moment not all
/// subsystems are implemented and the rest can be mocked with the [`DummySubsystem`].
#[derive(Debug, Clone, AllSubsystemsGen)]
pub struct AllSubsystems<
	CV = (), CB = (), SD = (), AD = (), AR = (), BS = (), BD = (), P = (),
	RA = (), AS = (), NB = (), CA = (), CG = (), CP = (), ApD = (), ApV = (),
	GS = (),
> {
	/// A candidate validation subsystem.
	pub candidate_validation: CV,
	/// A candidate backing subsystem.
	pub candidate_backing: CB,
	/// A statement distribution subsystem.
	pub statement_distribution: SD,
	/// An availability distribution subsystem.
	pub availability_distribution: AD,
	/// An availability recovery subsystem.
	pub availability_recovery: AR,
	/// A bitfield signing subsystem.
	pub bitfield_signing: BS,
	/// A bitfield distribution subsystem.
	pub bitfield_distribution: BD,
	/// A provisioner subsystem.
	pub provisioner: P,
	/// A runtime API subsystem.
	pub runtime_api: RA,
	/// An availability store subsystem.
	pub availability_store: AS,
	/// A network bridge subsystem.
	pub network_bridge: NB,
	/// A Chain API subsystem.
	pub chain_api: CA,
	/// A Collation Generation subsystem.
	pub collation_generation: CG,
	/// A Collator Protocol subsystem.
	pub collator_protocol: CP,
	/// An Approval Distribution subsystem.
	pub approval_distribution: ApD,
	/// An Approval Voting subsystem.
	pub approval_voting: ApV,
	/// A Connection Request Issuer subsystem.
	pub gossip_support: GS,
}

impl<CV, CB, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>
	AllSubsystems<CV, CB, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>
{
	/// Create a new instance of [`AllSubsystems`].
	///
	/// Each subsystem is set to [`DummySystem`].
	///
	///# Note
	///
	/// Because of a bug in rustc it is required that when calling this function,
	/// you provide a "random" type for the first generic parameter:
	///
	/// ```
	/// polkadot_overseer::AllSubsystems::<()>::dummy();
	/// ```
	pub fn dummy() -> AllSubsystems<
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
	> {
		AllSubsystems {
			candidate_validation: DummySubsystem,
			candidate_backing: DummySubsystem,
			statement_distribution: DummySubsystem,
			availability_distribution: DummySubsystem,
			availability_recovery: DummySubsystem,
			bitfield_signing: DummySubsystem,
			bitfield_distribution: DummySubsystem,
			provisioner: DummySubsystem,
			runtime_api: DummySubsystem,
			availability_store: DummySubsystem,
			network_bridge: DummySubsystem,
			chain_api: DummySubsystem,
			collation_generation: DummySubsystem,
			collator_protocol: DummySubsystem,
			approval_distribution: DummySubsystem,
			approval_voting: DummySubsystem,
			gossip_support: DummySubsystem,
		}
	}

	fn as_ref(&self) -> AllSubsystems<&'_ CV, &'_ CB, &'_ SD, &'_ AD, &'_ AR, &'_ BS, &'_ BD, &'_ P, &'_ RA, &'_ AS, &'_ NB, &'_ CA, &'_ CG, &'_ CP, &'_ ApD, &'_ ApV, &'_ GS> {
		AllSubsystems {
			candidate_validation: &self.candidate_validation,
			candidate_backing: &self.candidate_backing,
			statement_distribution: &self.statement_distribution,
			availability_distribution: &self.availability_distribution,
			availability_recovery: &self.availability_recovery,
			bitfield_signing: &self.bitfield_signing,
			bitfield_distribution: &self.bitfield_distribution,
			provisioner: &self.provisioner,
			runtime_api: &self.runtime_api,
			availability_store: &self.availability_store,
			network_bridge: &self.network_bridge,
			chain_api: &self.chain_api,
			collation_generation: &self.collation_generation,
			collator_protocol: &self.collator_protocol,
			approval_distribution: &self.approval_distribution,
			approval_voting: &self.approval_voting,
			gossip_support: &self.gossip_support,
		}
	}

	fn map_subsystems<M>(self, m: M)
		-> AllSubsystems<
			<M as MapSubsystem<CV>>::Output,
			<M as MapSubsystem<CB>>::Output,
			<M as MapSubsystem<SD>>::Output,
			<M as MapSubsystem<AD>>::Output,
			<M as MapSubsystem<AR>>::Output,
			<M as MapSubsystem<BS>>::Output,
			<M as MapSubsystem<BD>>::Output,
			<M as MapSubsystem<P>>::Output,
			<M as MapSubsystem<RA>>::Output,
			<M as MapSubsystem<AS>>::Output,
			<M as MapSubsystem<NB>>::Output,
			<M as MapSubsystem<CA>>::Output,
			<M as MapSubsystem<CG>>::Output,
			<M as MapSubsystem<CP>>::Output,
			<M as MapSubsystem<ApD>>::Output,
			<M as MapSubsystem<ApV>>::Output,
			<M as MapSubsystem<GS>>::Output,
		>
	where
		M: MapSubsystem<CV>,
		M: MapSubsystem<CB>,
		M: MapSubsystem<SD>,
		M: MapSubsystem<AD>,
		M: MapSubsystem<AR>,
		M: MapSubsystem<BS>,
		M: MapSubsystem<BD>,
		M: MapSubsystem<P>,
		M: MapSubsystem<RA>,
		M: MapSubsystem<AS>,
		M: MapSubsystem<NB>,
		M: MapSubsystem<CA>,
		M: MapSubsystem<CG>,
		M: MapSubsystem<CP>,
		M: MapSubsystem<ApD>,
		M: MapSubsystem<ApV>,
		M: MapSubsystem<GS>,
	{
		AllSubsystems {
			candidate_validation: m.map_subsystem(self.candidate_validation),
			candidate_backing: m.map_subsystem(self.candidate_backing),
			statement_distribution: m.map_subsystem(self.statement_distribution),
			availability_distribution: m.map_subsystem(self.availability_distribution),
			availability_recovery: m.map_subsystem(self.availability_recovery),
			bitfield_signing: m.map_subsystem(self.bitfield_signing),
			bitfield_distribution: m.map_subsystem(self.bitfield_distribution),
			provisioner: m.map_subsystem(self.provisioner),
			runtime_api: m.map_subsystem(self.runtime_api),
			availability_store: m.map_subsystem(self.availability_store),
			network_bridge: m.map_subsystem(self.network_bridge),
			chain_api: m.map_subsystem(self.chain_api),
			collation_generation: m.map_subsystem(self.collation_generation),
			collator_protocol: m.map_subsystem(self.collator_protocol),
			approval_distribution: m.map_subsystem(self.approval_distribution),
			approval_voting: m.map_subsystem(self.approval_voting),
			gossip_support: m.map_subsystem(self.gossip_support),
		}
	}
}

type AllSubsystemsSame<T> = AllSubsystems<
	T, T, T, T, T,
	T, T, T, T, T,
	T, T, T, T, T,
	T, T,
>;

/// A type of messages that are sent from [`Subsystem`] to [`Overseer`].
///
/// It wraps a system-wide [`AllMessages`] type that represents all possible
/// messages in the system.
///
/// [`AllMessages`]: enum.AllMessages.html
/// [`Subsystem`]: trait.Subsystem.html
/// [`Overseer`]: struct.Overseer.html
enum ToOverseer {
	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the overseer and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnJob {
		name: &'static str,
		s: BoxFuture<'static, ()>,
	},

	/// Same as `SpawnJob` but for blocking tasks to be executed on a
	/// dedicated thread pool.
	SpawnBlockingJob {
		name: &'static str,
		s: BoxFuture<'static, ()>,
	},
}

/// An event telling the `Overseer` on the particular block
/// that has been imported or finalized.
///
/// This structure exists solely for the purposes of decoupling
/// `Overseer` code from the client code and the necessity to call
/// `HeaderBackend::block_number_from_id()`.
#[derive(Debug, Clone)]
pub struct BlockInfo {
	/// hash of the block.
	pub hash: Hash,
	/// hash of the parent block.
	pub parent_hash: Hash,
	/// block's number.
	pub number: BlockNumber,
}

impl From<BlockImportNotification<Block>> for BlockInfo {
	fn from(n: BlockImportNotification<Block>) -> Self {
		BlockInfo {
			hash: n.hash,
			parent_hash: n.header.parent_hash,
			number: n.header.number,
		}
	}
}

impl From<FinalityNotification<Block>> for BlockInfo {
	fn from(n: FinalityNotification<Block>) -> Self {
		BlockInfo {
			hash: n.hash,
			parent_hash: n.header.parent_hash,
			number: n.header.number,
		}
	}
}

/// Some event from the outer world.
enum Event {
	BlockImported(BlockInfo),
	BlockFinalized(BlockInfo),
	MsgToSubsystem(AllMessages),
	ExternalRequest(ExternalRequest),
	Stop,
}

/// Some request from outer world.
enum ExternalRequest {
	WaitForActivation {
		hash: Hash,
		response_channel: oneshot::Sender<SubsystemResult<()>>,
	},
}

/// A handler used to communicate with the [`Overseer`].
///
/// [`Overseer`]: struct.Overseer.html
#[derive(Clone)]
pub struct OverseerHandler {
	events_tx: metered::MeteredSender<Event>,
}

impl OverseerHandler {
	/// Inform the `Overseer` that that some block was imported.
	pub async fn block_imported(&mut self, block: BlockInfo) {
		self.send_and_log_error(Event::BlockImported(block)).await
	}

	/// Send some message to one of the `Subsystem`s.
	pub async fn send_msg(&mut self, msg: impl Into<AllMessages>) {
		self.send_and_log_error(Event::MsgToSubsystem(msg.into())).await
	}

	/// Inform the `Overseer` that some block was finalized.
	pub async fn block_finalized(&mut self, block: BlockInfo) {
		self.send_and_log_error(Event::BlockFinalized(block)).await
	}

	/// Wait for a block with the given hash to be in the active-leaves set.
	///
	/// The response channel responds if the hash was activated and is closed if the hash was deactivated.
	/// Note that due the fact the overseer doesn't store the whole active-leaves set, only deltas,
	/// the response channel may never return if the hash was deactivated before this call.
	/// In this case, it's the caller's responsibility to ensure a timeout is set.
	pub async fn wait_for_activation(&mut self, hash: Hash, response_channel: oneshot::Sender<SubsystemResult<()>>) {
		self.send_and_log_error(Event::ExternalRequest(ExternalRequest::WaitForActivation {
			hash,
			response_channel
		})).await
	}

	/// Tell `Overseer` to shutdown.
	pub async fn stop(&mut self) {
		self.send_and_log_error(Event::Stop).await
	}

	async fn send_and_log_error(&mut self, event: Event) {
		if self.events_tx.send(event).await.is_err() {
			tracing::info!(target: LOG_TARGET, "Failed to send an event to Overseer");
		}
	}
}

/// Glues together the [`Overseer`] and `BlockchainEvents` by forwarding
/// import and finality notifications into the [`OverseerHandler`].
///
/// [`Overseer`]: struct.Overseer.html
/// [`OverseerHandler`]: struct.OverseerHandler.html
pub async fn forward_events<P: BlockchainEvents<Block>>(
	client: Arc<P>,
	mut handler: OverseerHandler,
) {
	let mut finality = client.finality_notification_stream();
	let mut imports = client.import_notification_stream();

	loop {
		select! {
			f = finality.next() => {
				match f {
					Some(block) => {
						handler.block_finalized(block.into()).await;
					}
					None => break,
				}
			},
			i = imports.next() => {
				match i {
					Some(block) => {
						handler.block_imported(block.into()).await;
					}
					None => break,
				}
			},
			complete => break,
		}
	}
}

impl Debug for ToOverseer {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ToOverseer::SpawnJob { .. } => write!(f, "OverseerMessage::Spawn(..)"),
			ToOverseer::SpawnBlockingJob { .. } => write!(f, "OverseerMessage::SpawnBlocking(..)")
		}
	}
}

/// A running instance of some [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
struct SubsystemInstance<M> {
	tx_signal: metered::MeteredSender<OverseerSignal>,
	tx_bounded: metered::MeteredSender<MessagePacket<M>>,
	meters: SubsystemMeters,
	signals_received: usize,
	name: &'static str,
}

#[derive(Debug)]
struct MessagePacket<T> {
	signals_received: usize,
	message: T,
}

fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
	MessagePacket {
		signals_received,
		message,
	}
}

// The channels held by every subsystem to communicate with every other subsystem.
#[derive(Debug, Clone)]
struct ChannelsOut {
	candidate_validation: metered::MeteredSender<MessagePacket<CandidateValidationMessage>>,
	candidate_backing: metered::MeteredSender<MessagePacket<CandidateBackingMessage>>,
	statement_distribution: metered::MeteredSender<MessagePacket<StatementDistributionMessage>>,
	availability_distribution: metered::MeteredSender<MessagePacket<AvailabilityDistributionMessage>>,
	availability_recovery: metered::MeteredSender<MessagePacket<AvailabilityRecoveryMessage>>,
	bitfield_signing: metered::MeteredSender<MessagePacket<BitfieldSigningMessage>>,
	bitfield_distribution: metered::MeteredSender<MessagePacket<BitfieldDistributionMessage>>,
	provisioner: metered::MeteredSender<MessagePacket<ProvisionerMessage>>,
	runtime_api: metered::MeteredSender<MessagePacket<RuntimeApiMessage>>,
	availability_store: metered::MeteredSender<MessagePacket<AvailabilityStoreMessage>>,
	network_bridge: metered::MeteredSender<MessagePacket<NetworkBridgeMessage>>,
	chain_api: metered::MeteredSender<MessagePacket<ChainApiMessage>>,
	collation_generation: metered::MeteredSender<MessagePacket<CollationGenerationMessage>>,
	collator_protocol: metered::MeteredSender<MessagePacket<CollatorProtocolMessage>>,
	approval_distribution: metered::MeteredSender<MessagePacket<ApprovalDistributionMessage>>,
	approval_voting: metered::MeteredSender<MessagePacket<ApprovalVotingMessage>>,
	gossip_support: metered::MeteredSender<MessagePacket<GossipSupportMessage>>,

	candidate_validation_unbounded: metered::UnboundedMeteredSender<MessagePacket<CandidateValidationMessage>>,
	candidate_backing_unbounded: metered::UnboundedMeteredSender<MessagePacket<CandidateBackingMessage>>,
	statement_distribution_unbounded: metered::UnboundedMeteredSender<MessagePacket<StatementDistributionMessage>>,
	availability_distribution_unbounded: metered::UnboundedMeteredSender<MessagePacket<AvailabilityDistributionMessage>>,
	availability_recovery_unbounded: metered::UnboundedMeteredSender<MessagePacket<AvailabilityRecoveryMessage>>,
	bitfield_signing_unbounded: metered::UnboundedMeteredSender<MessagePacket<BitfieldSigningMessage>>,
	bitfield_distribution_unbounded: metered::UnboundedMeteredSender<MessagePacket<BitfieldDistributionMessage>>,
	provisioner_unbounded: metered::UnboundedMeteredSender<MessagePacket<ProvisionerMessage>>,
	runtime_api_unbounded: metered::UnboundedMeteredSender<MessagePacket<RuntimeApiMessage>>,
	availability_store_unbounded: metered::UnboundedMeteredSender<MessagePacket<AvailabilityStoreMessage>>,
	network_bridge_unbounded: metered::UnboundedMeteredSender<MessagePacket<NetworkBridgeMessage>>,
	chain_api_unbounded: metered::UnboundedMeteredSender<MessagePacket<ChainApiMessage>>,
	collation_generation_unbounded: metered::UnboundedMeteredSender<MessagePacket<CollationGenerationMessage>>,
	collator_protocol_unbounded: metered::UnboundedMeteredSender<MessagePacket<CollatorProtocolMessage>>,
	approval_distribution_unbounded: metered::UnboundedMeteredSender<MessagePacket<ApprovalDistributionMessage>>,
	approval_voting_unbounded: metered::UnboundedMeteredSender<MessagePacket<ApprovalVotingMessage>>,
	gossip_support_unbounded: metered::UnboundedMeteredSender<MessagePacket<GossipSupportMessage>>,
}

impl ChannelsOut {
	async fn send_and_log_error(
		&mut self,
		signals_received: usize,
		message: AllMessages,
	) {
		let res = match message {
			AllMessages::CandidateValidation(msg) => {
				self.candidate_validation.send(make_packet(signals_received, msg)).await
			},
			AllMessages::CandidateBacking(msg) => {
				self.candidate_backing.send(make_packet(signals_received, msg)).await
			},
			AllMessages::StatementDistribution(msg) => {
				self.statement_distribution.send(make_packet(signals_received, msg)).await
			},
			AllMessages::AvailabilityDistribution(msg) => {
				self.availability_distribution.send(make_packet(signals_received, msg)).await
			},
			AllMessages::AvailabilityRecovery(msg) => {
				self.availability_recovery.send(make_packet(signals_received, msg)).await
			},
			AllMessages::BitfieldDistribution(msg) => {
				self.bitfield_distribution.send(make_packet(signals_received, msg)).await
			},
			AllMessages::BitfieldSigning(msg) => {
				self.bitfield_signing.send(make_packet(signals_received, msg)).await
			},
			AllMessages::Provisioner(msg) => {
				self.provisioner.send(make_packet(signals_received, msg)).await
			},
			AllMessages::RuntimeApi(msg) => {
				self.runtime_api.send(make_packet(signals_received, msg)).await
			},
			AllMessages::AvailabilityStore(msg) => {
				self.availability_store.send(make_packet(signals_received, msg)).await
			},
			AllMessages::NetworkBridge(msg) => {
				self.network_bridge.send(make_packet(signals_received, msg)).await
			},
			AllMessages::ChainApi(msg) => {
				self.chain_api.send(make_packet(signals_received, msg)).await
			},
			AllMessages::CollationGeneration(msg) => {
				self.collation_generation.send(make_packet(signals_received, msg)).await
			},
			AllMessages::CollatorProtocol(msg) => {
				self.collator_protocol.send(make_packet(signals_received, msg)).await
			},
			AllMessages::ApprovalDistribution(msg) => {
				self.approval_distribution.send(make_packet(signals_received, msg)).await
			},
			AllMessages::ApprovalVoting(msg) => {
				self.approval_voting.send(make_packet(signals_received, msg)).await
			},
			AllMessages::GossipSupport(msg) => {
				self.gossip_support.send(make_packet(signals_received, msg)).await
			},
			AllMessages::DisputeCoordinator(_) => Ok(()),
			AllMessages::DisputeParticipation(_) => Ok(()),
		};

		if res.is_err() {
			tracing::debug!(
				target: LOG_TARGET,
				"Failed to send a message to another subsystem",
			);
		}
	}


	fn send_unbounded_and_log_error(
		&self,
		signals_received: usize,
		message: AllMessages,
	) {
		let res = match message {
			AllMessages::CandidateValidation(msg) => {
				self.candidate_validation_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::CandidateBacking(msg) => {
				self.candidate_backing_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::StatementDistribution(msg) => {
				self.statement_distribution_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::AvailabilityDistribution(msg) => {
				self.availability_distribution_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::AvailabilityRecovery(msg) => {
				self.availability_recovery_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::BitfieldDistribution(msg) => {
				self.bitfield_distribution_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::BitfieldSigning(msg) => {
				self.bitfield_signing_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::Provisioner(msg) => {
				self.provisioner_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::RuntimeApi(msg) => {
				self.runtime_api_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::AvailabilityStore(msg) => {
				self.availability_store_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::NetworkBridge(msg) => {
				self.network_bridge_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::ChainApi(msg) => {
				self.chain_api_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::CollationGeneration(msg) => {
				self.collation_generation_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::CollatorProtocol(msg) => {
				self.collator_protocol_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::ApprovalDistribution(msg) => {
				self.approval_distribution_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::ApprovalVoting(msg) => {
				self.approval_voting_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::GossipSupport(msg) => {
				self.gossip_support_unbounded
					.unbounded_send(make_packet(signals_received, msg))
					.map_err(|e| e.into_send_error())
			},
			AllMessages::DisputeCoordinator(_) => Ok(()),
			AllMessages::DisputeParticipation(_) => Ok(()),
		};

		if res.is_err() {
			tracing::debug!(
				target: LOG_TARGET,
				"Failed to send a message to another subsystem",
			);
		}
	}
}

type SubsystemIncomingMessages<M> = stream::Select<
	metered::MeteredReceiver<MessagePacket<M>>,
	metered::UnboundedMeteredReceiver<MessagePacket<M>>,
>;

#[derive(Debug, Default, Clone)]
struct SignalsReceived(Arc<AtomicUsize>);

impl SignalsReceived {
	fn load(&self) -> usize {
		self.0.load(atomic::Ordering::SeqCst)
	}

	fn inc(&self) {
		self.0.fetch_add(1, atomic::Ordering::SeqCst);
	}
}

/// A sender from subsystems to other subsystems.
#[derive(Debug, Clone)]
pub struct OverseerSubsystemSender {
	channels: ChannelsOut,
	signals_received: SignalsReceived,
}

#[async_trait::async_trait]
impl SubsystemSender for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: AllMessages) {
		self.channels.send_and_log_error(self.signals_received.load(), msg).await;
	}

	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		// This can definitely be optimized if necessary.
		for msg in msgs {
			self.send_message(msg).await;
		}
	}

	fn send_unbounded_message(&mut self, msg: AllMessages) {
		self.channels.send_unbounded_and_log_error(self.signals_received.load(), msg);
	}
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or to spawn it's [`SubsystemJob`]s.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
#[derive(Debug)]
pub struct OverseerSubsystemContext<M>{
	signals: metered::MeteredReceiver<OverseerSignal>,
	messages: SubsystemIncomingMessages<M>,
	to_subsystems: OverseerSubsystemSender,
	to_overseer: metered::UnboundedMeteredSender<ToOverseer>,
	signals_received: SignalsReceived,
	pending_incoming: Option<(usize, M)>,
	metrics: Metrics,
}

impl<M> OverseerSubsystemContext<M> {
	/// Create a new `OverseerSubsystemContext`.
	fn new(
		signals: metered::MeteredReceiver<OverseerSignal>,
		messages: SubsystemIncomingMessages<M>,
		to_subsystems: ChannelsOut,
		to_overseer: metered::UnboundedMeteredSender<ToOverseer>,
		metrics: Metrics,
	) -> Self {
		let signals_received = SignalsReceived::default();
		OverseerSubsystemContext {
			signals,
			messages,
			to_subsystems: OverseerSubsystemSender {
				channels: to_subsystems,
				signals_received: signals_received.clone(),
			},
			to_overseer,
			signals_received,
			pending_incoming: None,
			metrics,
		 }
	}

	/// Create a new `OverseerSubsystemContext` with no metering.
	///
	/// Intended for tests.
	#[allow(unused)]
	fn new_unmetered(
		signals: metered::MeteredReceiver<OverseerSignal>,
		messages: SubsystemIncomingMessages<M>,
		to_subsystems: ChannelsOut,
		to_overseer: metered::UnboundedMeteredSender<ToOverseer>,
	) -> Self {
		let metrics = Metrics::default();
		OverseerSubsystemContext::new(signals, messages, to_subsystems, to_overseer, metrics)
	}
}

#[async_trait::async_trait]
impl<M: Send + 'static> SubsystemContext for OverseerSubsystemContext<M> {
	type Message = M;
	type Sender = OverseerSubsystemSender;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<M>>, ()> {
		match poll!(self.recv()) {
			Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
			Poll::Pending => Ok(None),
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<M>> {
		loop {
			// If we have a message pending an overseer signal, we only poll for signals
			// in the meantime.
			if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
				if needs_signals_received <= self.signals_received.load() {
					return Ok(FromOverseer::Communication { msg });
				} else {
					self.pending_incoming = Some((needs_signals_received, msg));

					// wait for next signal.
					let signal = self.signals.next().await
						.ok_or(SubsystemError::Context(
							"Signal channel is terminated and empty."
							.to_owned()
						))?;

					self.signals_received.inc();
					return Ok(FromOverseer::Signal(signal))
				}
			}

			let mut await_message = self.messages.next().fuse();
			let mut await_signal = self.signals.next().fuse();
			let signals_received = self.signals_received.load();
			let pending_incoming = &mut self.pending_incoming;

			// Otherwise, wait for the next signal or incoming message.
			let from_overseer = futures::select_biased! {
				signal = await_signal => {
					let signal = signal
						.ok_or(SubsystemError::Context(
							"Signal channel is terminated and empty."
							.to_owned()
						))?;

					FromOverseer::Signal(signal)
				}
				msg = await_message => {
					let packet = msg
						.ok_or(SubsystemError::Context(
							"Message channel is terminated and empty."
							.to_owned()
						))?;

					if packet.signals_received > signals_received {
						// wait until we've received enough signals to return this message.
						*pending_incoming = Some((packet.signals_received, packet.message));
						continue;
					} else {
						// we know enough to return this message.
						FromOverseer::Communication { msg: packet.message}
					}
				}
			};

			if let FromOverseer::Signal(_) = from_overseer {
				self.signals_received.inc();
			}

			return Ok(from_overseer);
		}
	}

	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
		-> SubsystemResult<()>
	{
		self.to_overseer.send(ToOverseer::SpawnJob {
			name,
			s,
		}).await.map_err(Into::into)
	}

	async fn spawn_blocking(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
		-> SubsystemResult<()>
	{
		self.to_overseer.send(ToOverseer::SpawnBlockingJob {
			name,
			s,
		}).await.map_err(Into::into)
	}

	fn sender(&mut self) -> &mut OverseerSubsystemSender {
		&mut self.to_subsystems
	}
}

/// A subsystem that we oversee.
///
/// Ties together the [`Subsystem`] itself and it's running instance
/// (which may be missing if the [`Subsystem`] is not running at the moment
/// for whatever reason).
///
/// [`Subsystem`]: trait.Subsystem.html
struct OverseenSubsystem<M> {
	instance: Option<SubsystemInstance<M>>,
}

impl<M> OverseenSubsystem<M> {
	/// Send a message to the wrapped subsystem.
	///
	/// If the inner `instance` is `None`, nothing is happening.
	async fn send_message(&mut self, msg: M) -> SubsystemResult<()> {
		const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

		if let Some(ref mut instance) = self.instance {
			match instance.tx_bounded.send(MessagePacket {
				signals_received: instance.signals_received,
				message: msg.into()
			}).timeout(MESSAGE_TIMEOUT).await
			{
				None => {
					tracing::error!(target: LOG_TARGET, "Subsystem {} appears unresponsive.", instance.name);
					Err(SubsystemError::SubsystemStalled(instance.name))
				}
				Some(res) => res.map_err(Into::into),
			}
		} else {
			Ok(())
		}
	}

	/// Send a signal to the wrapped subsystem.
	///
	/// If the inner `instance` is `None`, nothing is happening.
	async fn send_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
		const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);

		if let Some(ref mut instance) = self.instance {
			match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
				None => {
					tracing::error!(target: LOG_TARGET, "Subsystem {} appears unresponsive.", instance.name);
					Err(SubsystemError::SubsystemStalled(instance.name))
				}
				Some(res) => {
					let res = res.map_err(Into::into);
					if res.is_ok() {
						instance.signals_received += 1;
					}
					res
				}
			}
		} else {
			Ok(())
		}
	}
}

#[derive(Clone)]
struct SubsystemMeters {
	bounded: metered::Meter,
	unbounded: metered::Meter,
	signals: metered::Meter,
}

impl SubsystemMeters {
	fn read(&self) -> SubsystemMeterReadouts {
		SubsystemMeterReadouts {
			bounded: self.bounded.read(),
			unbounded: self.unbounded.read(),
			signals: self.signals.read(),
		}
	}
}

struct SubsystemMeterReadouts {
	bounded: metered::Readout,
	unbounded: metered::Readout,
	signals: metered::Readout,
}


/// Store 2 days worth of blocks, not accounting for forks,
/// in the LRU cache. Assumes a 6-second block time.
const KNOWN_LEAVES_CACHE_SIZE: usize = 2 * 24 * 3600 / 6;

/// The `Overseer` itself.
pub struct Overseer<S, SupportsParachains> {
	/// Handles to all subsystems.
	subsystems: AllSubsystems<
		OverseenSubsystem<CandidateValidationMessage>,
		OverseenSubsystem<CandidateBackingMessage>,
		OverseenSubsystem<StatementDistributionMessage>,
		OverseenSubsystem<AvailabilityDistributionMessage>,
		OverseenSubsystem<AvailabilityRecoveryMessage>,
		OverseenSubsystem<BitfieldSigningMessage>,
		OverseenSubsystem<BitfieldDistributionMessage>,
		OverseenSubsystem<ProvisionerMessage>,
		OverseenSubsystem<RuntimeApiMessage>,
		OverseenSubsystem<AvailabilityStoreMessage>,
		OverseenSubsystem<NetworkBridgeMessage>,
		OverseenSubsystem<ChainApiMessage>,
		OverseenSubsystem<CollationGenerationMessage>,
		OverseenSubsystem<CollatorProtocolMessage>,
		OverseenSubsystem<ApprovalDistributionMessage>,
		OverseenSubsystem<ApprovalVotingMessage>,
		OverseenSubsystem<GossipSupportMessage>,
	>,

	/// Spawner to spawn tasks to.
	s: S,

	/// Here we keep handles to spawned subsystems to be notified when they terminate.
	running_subsystems: FuturesUnordered<BoxFuture<'static, SubsystemResult<()>>>,

	/// Gather running subsystems' outbound streams into one.
	to_overseer_rx: Fuse<metered::UnboundedMeteredReceiver<ToOverseer>>,

	/// Events that are sent to the overseer from the outside world
	events_rx: metered::MeteredReceiver<Event>,

	/// External listeners waiting for a hash to be in the active-leave set.
	activation_external_listeners: HashMap<Hash, Vec<oneshot::Sender<SubsystemResult<()>>>>,

	/// Stores the [`jaeger::Span`] per active leaf.
	span_per_active_leaf: HashMap<Hash, Arc<jaeger::Span>>,

	/// A set of leaves that `Overseer` starts working with.
	///
	/// Drained at the beginning of `run` and never used again.
	leaves: Vec<(Hash, BlockNumber)>,

	/// The set of the "active leaves".
	active_leaves: HashMap<Hash, BlockNumber>,

	/// An implementation for checking whether a header supports parachain consensus.
	supports_parachains: SupportsParachains,

	/// An LRU cache for keeping track of relay-chain heads that have already been seen.
	known_leaves: LruCache<Hash, ()>,

	/// Various Prometheus metrics.
	metrics: Metrics,
}

/// Overseer Prometheus metrics.
#[derive(Clone)]
struct MetricsInner {
	activated_heads_total: prometheus::Counter<prometheus::U64>,
	deactivated_heads_total: prometheus::Counter<prometheus::U64>,
	messages_relayed_total: prometheus::Counter<prometheus::U64>,
	to_subsystem_bounded_sent: prometheus::GaugeVec<prometheus::U64>,
	to_subsystem_bounded_received: prometheus::GaugeVec<prometheus::U64>,
	to_subsystem_unbounded_sent: prometheus::GaugeVec<prometheus::U64>,
	to_subsystem_unbounded_received: prometheus::GaugeVec<prometheus::U64>,
	signals_sent: prometheus::GaugeVec<prometheus::U64>,
	signals_received: prometheus::GaugeVec<prometheus::U64>,
}

#[derive(Default, Clone)]
struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_head_activated(&self) {
		if let Some(metrics) = &self.0 {
			metrics.activated_heads_total.inc();
		}
	}

	fn on_head_deactivated(&self) {
		if let Some(metrics) = &self.0 {
			metrics.deactivated_heads_total.inc();
		}
	}

	fn on_message_relayed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.messages_relayed_total.inc();
		}
	}

	fn channel_fill_level_snapshot(
		&self,
		to_subsystem: AllSubsystemsSame<(&'static str, SubsystemMeterReadouts)>,
	) {
		self.0.as_ref().map(|metrics| {
			to_subsystem.map_subsystems(
				|(name, readouts): (_, SubsystemMeterReadouts)| {
					metrics.to_subsystem_bounded_sent.with_label_values(&[name])
						.set(readouts.bounded.sent as u64);

					metrics.to_subsystem_bounded_received.with_label_values(&[name])
						.set(readouts.bounded.received as u64);

					metrics.to_subsystem_unbounded_sent.with_label_values(&[name])
						.set(readouts.unbounded.sent as u64);

					metrics.to_subsystem_unbounded_received.with_label_values(&[name])
						.set(readouts.unbounded.received as u64);

					metrics.signals_sent.with_label_values(&[name])
						.set(readouts.signals.sent as u64);

					metrics.signals_received.with_label_values(&[name])
						.set(readouts.signals.received as u64);
				});
		});
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			activated_heads_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_activated_heads_total",
					"Number of activated heads."
				)?,
				registry,
			)?,
			deactivated_heads_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_deactivated_heads_total",
					"Number of deactivated heads."
				)?,
				registry,
			)?,
			messages_relayed_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_messages_relayed_total",
					"Number of messages relayed by Overseer."
				)?,
				registry,
			)?,
			to_subsystem_bounded_sent: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_bounded_sent",
						"Number of elements sent to subsystems' bounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			to_subsystem_bounded_received: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_bounded_received",
						"Number of elements received by subsystems' bounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			to_subsystem_unbounded_sent: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_unbounded_sent",
						"Number of elements sent to subsystems' unbounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			to_subsystem_unbounded_received: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_subsystem_unbounded_received",
						"Number of elements received by subsystems' unbounded queues",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			signals_sent: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_overseer_signals_sent",
						"Number of signals sent by overseer to subsystems",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
			signals_received: prometheus::register(
				prometheus::GaugeVec::<prometheus::U64>::new(
					prometheus::Opts::new(
						"parachain_overseer_signals_received",
						"Number of signals received by subsystems from overseer",
					),
					&[
						"subsystem_name",
					],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

impl fmt::Debug for Metrics {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("Metrics {{...}}")
	}
}

impl<S, SupportsParachains> Overseer<S, SupportsParachains>
where
	S: SpawnNamed,
	SupportsParachains: HeadSupportsParachains,
{
	/// Create a new instance of the `Overseer` with a fixed set of [`Subsystem`]s.
	///
	/// ```text
	///                  +------------------------------------+
	///                  |            Overseer                |
	///                  +------------------------------------+
	///                    /            |             |      \
	///      ................. subsystems...................................
	///      . +-----------+    +-----------+   +----------+   +---------+ .
	///      . |           |    |           |   |          |   |         | .
	///      . +-----------+    +-----------+   +----------+   +---------+ .
	///      ...............................................................
	///                              |
	///                        probably `spawn`
	///                            a `job`
	///                              |
	///                              V
	///                         +-----------+
	///                         |           |
	///                         +-----------+
	///
	/// ```
	///
	/// [`Subsystem`]: trait.Subsystem.html
	///
	/// # Example
	///
	/// The [`Subsystems`] may be any type as long as they implement an expected interface.
	/// Here, we create a mock validation subsystem and a few dummy ones and start the `Overseer` with them.
	/// For the sake of simplicity the termination of the example is done with a timeout.
	/// ```
	/// # use std::time::Duration;
	/// # use futures::{executor, pin_mut, select, FutureExt};
	/// # use futures_timer::Delay;
	/// # use polkadot_overseer::{Overseer, HeadSupportsParachains, AllSubsystems};
	/// # use polkadot_primitives::v1::Hash;
	/// # use polkadot_subsystem::{
	/// #     Subsystem, DummySubsystem, SpawnedSubsystem, SubsystemContext,
	/// #     messages::CandidateValidationMessage,
	/// # };
	///
	/// struct ValidationSubsystem;
	///
	/// impl<C> Subsystem<C> for ValidationSubsystem
	///     where C: SubsystemContext<Message=CandidateValidationMessage>
	/// {
	///     fn start(
	///         self,
	///         mut ctx: C,
	///     ) -> SpawnedSubsystem {
	///         SpawnedSubsystem {
	///             name: "validation-subsystem",
	///             future: Box::pin(async move {
	///                 loop {
	///                     Delay::new(Duration::from_secs(1)).await;
	///                 }
	///             }),
	///         }
	///     }
	/// }
	///
	/// # fn main() { executor::block_on(async move {
	///
	/// struct AlwaysSupportsParachains;
	/// impl HeadSupportsParachains for AlwaysSupportsParachains {
	///      fn head_supports_parachains(&self, _head: &Hash) -> bool { true }
	/// }
	/// let spawner = sp_core::testing::TaskExecutor::new();
	/// let all_subsystems = AllSubsystems::<()>::dummy().replace_candidate_validation(ValidationSubsystem);
	/// let (overseer, _handler) = Overseer::new(
	///     vec![],
	///     all_subsystems,
	///     None,
	///     AlwaysSupportsParachains,
	///     spawner,
	/// ).unwrap();
	///
	/// let timer = Delay::new(Duration::from_millis(50)).fuse();
	///
	/// let overseer_fut = overseer.run().fuse();
	/// pin_mut!(timer);
	/// pin_mut!(overseer_fut);
	///
	/// select! {
	///     _ = overseer_fut => (),
	///     _ = timer => (),
	/// }
	/// #
	/// # }); }
	/// ```
	pub fn new<CV, CB, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>(
		leaves: impl IntoIterator<Item = BlockInfo>,
		all_subsystems: AllSubsystems<CV, CB, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>,
		prometheus_registry: Option<&prometheus::Registry>,
		supports_parachains: SupportsParachains,
		mut s: S,
	) -> SubsystemResult<(Self, OverseerHandler)>
	where
		CV: Subsystem<OverseerSubsystemContext<CandidateValidationMessage>> + Send,
		CB: Subsystem<OverseerSubsystemContext<CandidateBackingMessage>> + Send,
		SD: Subsystem<OverseerSubsystemContext<StatementDistributionMessage>> + Send,
		AD: Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>> + Send,
		AR: Subsystem<OverseerSubsystemContext<AvailabilityRecoveryMessage>> + Send,
		BS: Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>> + Send,
		BD: Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>> + Send,
		P: Subsystem<OverseerSubsystemContext<ProvisionerMessage>> + Send,
		RA: Subsystem<OverseerSubsystemContext<RuntimeApiMessage>> + Send,
		AS: Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>> + Send,
		NB: Subsystem<OverseerSubsystemContext<NetworkBridgeMessage>> + Send,
		CA: Subsystem<OverseerSubsystemContext<ChainApiMessage>> + Send,
		CG: Subsystem<OverseerSubsystemContext<CollationGenerationMessage>> + Send,
		CP: Subsystem<OverseerSubsystemContext<CollatorProtocolMessage>> + Send,
		ApD: Subsystem<OverseerSubsystemContext<ApprovalDistributionMessage>> + Send,
		ApV: Subsystem<OverseerSubsystemContext<ApprovalVotingMessage>> + Send,
		GS: Subsystem<OverseerSubsystemContext<GossipSupportMessage>> + Send,
	{
		let (events_tx, events_rx) = metered::channel(CHANNEL_CAPACITY);

		let handler = OverseerHandler {
			events_tx: events_tx.clone(),
		};

		let metrics = <Metrics as metrics::Metrics>::register(prometheus_registry)?;

		let (to_overseer_tx, to_overseer_rx) = metered::unbounded();

		let mut running_subsystems = FuturesUnordered::new();

		let (candidate_validation_bounded_tx, candidate_validation_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (candidate_backing_bounded_tx, candidate_backing_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (statement_distribution_bounded_tx, statement_distribution_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (availability_distribution_bounded_tx, availability_distribution_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (availability_recovery_bounded_tx, availability_recovery_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (bitfield_signing_bounded_tx, bitfield_signing_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (bitfield_distribution_bounded_tx, bitfield_distribution_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (provisioner_bounded_tx, provisioner_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (runtime_api_bounded_tx, runtime_api_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (availability_store_bounded_tx, availability_store_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (network_bridge_bounded_tx, network_bridge_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (chain_api_bounded_tx, chain_api_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (collator_protocol_bounded_tx, collator_protocol_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (collation_generation_bounded_tx, collation_generation_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (approval_distribution_bounded_tx, approval_distribution_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (approval_voting_bounded_tx, approval_voting_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);
		let (gossip_support_bounded_tx, gossip_support_bounded_rx)
			= metered::channel(CHANNEL_CAPACITY);

		let (candidate_validation_unbounded_tx, candidate_validation_unbounded_rx)
			= metered::unbounded();
		let (candidate_backing_unbounded_tx, candidate_backing_unbounded_rx)
			= metered::unbounded();
		let (statement_distribution_unbounded_tx, statement_distribution_unbounded_rx)
			= metered::unbounded();
		let (availability_distribution_unbounded_tx, availability_distribution_unbounded_rx)
			= metered::unbounded();
		let (availability_recovery_unbounded_tx, availability_recovery_unbounded_rx)
			= metered::unbounded();
		let (bitfield_signing_unbounded_tx, bitfield_signing_unbounded_rx)
			= metered::unbounded();
		let (bitfield_distribution_unbounded_tx, bitfield_distribution_unbounded_rx)
			= metered::unbounded();
		let (provisioner_unbounded_tx, provisioner_unbounded_rx)
			= metered::unbounded();
		let (runtime_api_unbounded_tx, runtime_api_unbounded_rx)
			= metered::unbounded();
		let (availability_store_unbounded_tx, availability_store_unbounded_rx)
			= metered::unbounded();
		let (network_bridge_unbounded_tx, network_bridge_unbounded_rx)
			= metered::unbounded();
		let (chain_api_unbounded_tx, chain_api_unbounded_rx)
			= metered::unbounded();
		let (collator_protocol_unbounded_tx, collator_protocol_unbounded_rx)
			= metered::unbounded();
		let (collation_generation_unbounded_tx, collation_generation_unbounded_rx)
			= metered::unbounded();
		let (approval_distribution_unbounded_tx, approval_distribution_unbounded_rx)
			= metered::unbounded();
		let (approval_voting_unbounded_tx, approval_voting_unbounded_rx)
			= metered::unbounded();
		let (gossip_support_unbounded_tx, gossip_support_unbounded_rx)
			= metered::unbounded();

		let channels_out = ChannelsOut {
			candidate_validation: candidate_validation_bounded_tx.clone(),
			candidate_backing: candidate_backing_bounded_tx.clone(),
			statement_distribution: statement_distribution_bounded_tx.clone(),
			availability_distribution: availability_distribution_bounded_tx.clone(),
			availability_recovery: availability_recovery_bounded_tx.clone(),
			bitfield_signing: bitfield_signing_bounded_tx.clone(),
			bitfield_distribution: bitfield_distribution_bounded_tx.clone(),
			provisioner: provisioner_bounded_tx.clone(),
			runtime_api: runtime_api_bounded_tx.clone(),
			availability_store: availability_store_bounded_tx.clone(),
			network_bridge: network_bridge_bounded_tx.clone(),
			chain_api: chain_api_bounded_tx.clone(),
			collator_protocol: collator_protocol_bounded_tx.clone(),
			collation_generation: collation_generation_bounded_tx.clone(),
			approval_distribution: approval_distribution_bounded_tx.clone(),
			approval_voting: approval_voting_bounded_tx.clone(),
			gossip_support: gossip_support_bounded_tx.clone(),

			candidate_validation_unbounded: candidate_validation_unbounded_tx.clone(),
			candidate_backing_unbounded: candidate_backing_unbounded_tx.clone(),
			statement_distribution_unbounded: statement_distribution_unbounded_tx.clone(),
			availability_distribution_unbounded: availability_distribution_unbounded_tx.clone(),
			availability_recovery_unbounded: availability_recovery_unbounded_tx.clone(),
			bitfield_signing_unbounded: bitfield_signing_unbounded_tx.clone(),
			bitfield_distribution_unbounded: bitfield_distribution_unbounded_tx.clone(),
			provisioner_unbounded: provisioner_unbounded_tx.clone(),
			runtime_api_unbounded: runtime_api_unbounded_tx.clone(),
			availability_store_unbounded: availability_store_unbounded_tx.clone(),
			network_bridge_unbounded: network_bridge_unbounded_tx.clone(),
			chain_api_unbounded: chain_api_unbounded_tx.clone(),
			collator_protocol_unbounded: collator_protocol_unbounded_tx.clone(),
			collation_generation_unbounded: collation_generation_unbounded_tx.clone(),
			approval_distribution_unbounded: approval_distribution_unbounded_tx.clone(),
			approval_voting_unbounded: approval_voting_unbounded_tx.clone(),
			gossip_support_unbounded: gossip_support_unbounded_tx.clone(),
		};

		let candidate_validation_subsystem = spawn(
			&mut s,
			candidate_validation_bounded_tx,
			stream::select(candidate_validation_bounded_rx, candidate_validation_unbounded_rx),
			candidate_validation_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.candidate_validation,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let candidate_backing_subsystem = spawn(
			&mut s,
			candidate_backing_bounded_tx,
			stream::select(candidate_backing_bounded_rx, candidate_backing_unbounded_rx),
			candidate_backing_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.candidate_backing,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let statement_distribution_subsystem = spawn(
			&mut s,
			statement_distribution_bounded_tx,
			stream::select(statement_distribution_bounded_rx, statement_distribution_unbounded_rx),
			statement_distribution_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.statement_distribution,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let availability_distribution_subsystem = spawn(
			&mut s,
			availability_distribution_bounded_tx,
			stream::select(availability_distribution_bounded_rx, availability_distribution_unbounded_rx),
			availability_distribution_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.availability_distribution,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let availability_recovery_subsystem = spawn(
			&mut s,
			availability_recovery_bounded_tx,
			stream::select(availability_recovery_bounded_rx, availability_recovery_unbounded_rx),
			availability_recovery_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.availability_recovery,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let bitfield_signing_subsystem = spawn(
			&mut s,
			bitfield_signing_bounded_tx,
			stream::select(bitfield_signing_bounded_rx, bitfield_signing_unbounded_rx),
			bitfield_signing_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.bitfield_signing,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let bitfield_distribution_subsystem = spawn(
			&mut s,
			bitfield_distribution_bounded_tx,
			stream::select(bitfield_distribution_bounded_rx, bitfield_distribution_unbounded_rx),
			bitfield_distribution_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.bitfield_distribution,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let provisioner_subsystem = spawn(
			&mut s,
			provisioner_bounded_tx,
			stream::select(provisioner_bounded_rx, provisioner_unbounded_rx),
			provisioner_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.provisioner,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let runtime_api_subsystem = spawn(
			&mut s,
			runtime_api_bounded_tx,
			stream::select(runtime_api_bounded_rx, runtime_api_unbounded_rx),
			runtime_api_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.runtime_api,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let availability_store_subsystem = spawn(
			&mut s,
			availability_store_bounded_tx,
			stream::select(availability_store_bounded_rx, availability_store_unbounded_rx),
			availability_store_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.availability_store,
			&metrics,
			&mut running_subsystems,
			TaskKind::Blocking,
		)?;

		let network_bridge_subsystem = spawn(
			&mut s,
			network_bridge_bounded_tx,
			stream::select(network_bridge_bounded_rx, network_bridge_unbounded_rx),
			network_bridge_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.network_bridge,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let chain_api_subsystem = spawn(
			&mut s,
			chain_api_bounded_tx,
			stream::select(chain_api_bounded_rx, chain_api_unbounded_rx),
			chain_api_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.chain_api,
			&metrics,
			&mut running_subsystems,
			TaskKind::Blocking,
		)?;

		let collation_generation_subsystem = spawn(
			&mut s,
			collation_generation_bounded_tx,
			stream::select(collation_generation_bounded_rx, collation_generation_unbounded_rx),
			collation_generation_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.collation_generation,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let collator_protocol_subsystem = spawn(
			&mut s,
			collator_protocol_bounded_tx,
			stream::select(collator_protocol_bounded_rx, collator_protocol_unbounded_rx),
			collator_protocol_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.collator_protocol,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let approval_distribution_subsystem = spawn(
			&mut s,
			approval_distribution_bounded_tx,
			stream::select(approval_distribution_bounded_rx, approval_distribution_unbounded_rx),
			approval_distribution_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.approval_distribution,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let approval_voting_subsystem = spawn(
			&mut s,
			approval_voting_bounded_tx,
			stream::select(approval_voting_bounded_rx, approval_voting_unbounded_rx),
			approval_voting_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.approval_voting,
			&metrics,
			&mut running_subsystems,
			TaskKind::Blocking,
		)?;

		let gossip_support_subsystem = spawn(
			&mut s,
			gossip_support_bounded_tx,
			stream::select(gossip_support_bounded_rx, gossip_support_unbounded_rx),
			gossip_support_unbounded_tx.meter().clone(),
			channels_out.clone(),
			to_overseer_tx.clone(),
			all_subsystems.gossip_support,
			&metrics,
			&mut running_subsystems,
			TaskKind::Regular,
		)?;

		let leaves = leaves
			.into_iter()
			.map(|BlockInfo { hash, parent_hash: _, number }| (hash, number))
			.collect();

		let active_leaves = HashMap::new();
		let activation_external_listeners = HashMap::new();

		let subsystems = AllSubsystems {
			candidate_validation: candidate_validation_subsystem,
			candidate_backing: candidate_backing_subsystem,
			statement_distribution: statement_distribution_subsystem,
			availability_distribution: availability_distribution_subsystem,
			availability_recovery: availability_recovery_subsystem,
			bitfield_signing: bitfield_signing_subsystem,
			bitfield_distribution: bitfield_distribution_subsystem,
			provisioner: provisioner_subsystem,
			runtime_api: runtime_api_subsystem,
			availability_store: availability_store_subsystem,
			network_bridge: network_bridge_subsystem,
			chain_api: chain_api_subsystem,
			collation_generation: collation_generation_subsystem,
			collator_protocol: collator_protocol_subsystem,
			approval_distribution: approval_distribution_subsystem,
			approval_voting: approval_voting_subsystem,
			gossip_support: gossip_support_subsystem,
		};

		{
			struct ExtractNameAndMeters;
			impl<'a, T: 'a> MapSubsystem<&'a OverseenSubsystem<T>> for ExtractNameAndMeters {
				type Output = (&'static str, SubsystemMeters);

				fn map_subsystem(&self, subsystem: &'a OverseenSubsystem<T>) -> Self::Output {
					let instance = subsystem.instance.as_ref()
						.expect("Extraction is done directly after spawning when subsystems\
						have not concluded; qed");

					(
						instance.name,
						instance.meters.clone(),
					)
				}
			}

			let subsystem_meters = subsystems.as_ref().map_subsystems(ExtractNameAndMeters);
			let metronome_metrics = metrics.clone();
			let metronome = Metronome::new(std::time::Duration::from_millis(950))
				.for_each(move |_| {
					let subsystem_meters = subsystem_meters.as_ref()
						.map_subsystems(|&(name, ref meters): &(_, SubsystemMeters)| (name, meters.read()));

					// We combine the amount of messages from subsystems to the overseer
					// as well as the amount of messages from external sources to the overseer
					// into one to_overseer value.
					metronome_metrics.channel_fill_level_snapshot(subsystem_meters);

					async move {
						()
					}
				});
			s.spawn("metrics_metronome", Box::pin(metronome));
		}

		let this = Self {
			subsystems,
			s,
			running_subsystems,
			to_overseer_rx: to_overseer_rx.fuse(),
			events_rx,
			activation_external_listeners,
			leaves,
			active_leaves,
			metrics,
			span_per_active_leaf: Default::default(),
			known_leaves: LruCache::new(KNOWN_LEAVES_CACHE_SIZE),
			supports_parachains,
		};

		Ok((this, handler))
	}

	// Stop the overseer.
	async fn stop(mut self) {
		let _ = self.subsystems.candidate_validation.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.candidate_backing.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.statement_distribution.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.availability_distribution.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.availability_recovery.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.bitfield_signing.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.bitfield_distribution.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.provisioner.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.runtime_api.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.availability_store.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.network_bridge.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.chain_api.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.collator_protocol.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.collation_generation.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.approval_distribution.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.approval_voting.send_signal(OverseerSignal::Conclude).await;
		let _ = self.subsystems.gossip_support.send_signal(OverseerSignal::Conclude).await;

		let mut stop_delay = Delay::new(Duration::from_secs(STOP_DELAY)).fuse();

		loop {
			select! {
				_ = self.running_subsystems.next() => {
					if self.running_subsystems.is_empty() {
						break;
					}
				},
				_ = stop_delay => break,
				complete => break,
			}
		}
	}

	/// Run the `Overseer`.
	pub async fn run(mut self) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		for (hash, number) in std::mem::take(&mut self.leaves) {
			let _ = self.active_leaves.insert(hash, number);
			if let Some((span, status)) = self.on_head_activated(&hash, None) {
				update.activated.push(ActivatedLeaf {
					hash,
					number,
					status,
					span,
				});
			}
		}

		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
		}

		loop {
			select! {
				msg = self.events_rx.next().fuse() => {
					let msg = if let Some(msg) = msg {
						msg
					} else {
						continue
					};

					match msg {
						Event::MsgToSubsystem(msg) => {
							self.route_message(msg.into()).await?;
						}
						Event::Stop => {
							self.stop().await;
							return Ok(());
						}
						Event::BlockImported(block) => {
							self.block_imported(block).await?;
						}
						Event::BlockFinalized(block) => {
							self.block_finalized(block).await?;
						}
						Event::ExternalRequest(request) => {
							self.handle_external_request(request);
						}
					}
				},
				msg = self.to_overseer_rx.next() => {
					let msg = match msg {
						Some(m) => m,
						None => {
							// This is a fused stream so we will shut down after receiving all
							// shutdown notifications.
							continue
						}
					};

					match msg {
						ToOverseer::SpawnJob { name, s } => {
							self.spawn_job(name, s);
						}
						ToOverseer::SpawnBlockingJob { name, s } => {
							self.spawn_blocking_job(name, s);
						}
					}
				},
				res = self.running_subsystems.next().fuse() => {
					let finished = if let Some(finished) = res {
						finished
					} else {
						continue
					};

					tracing::error!(target: LOG_TARGET, subsystem = ?finished, "subsystem finished unexpectedly");
					self.stop().await;
					return finished;
				},
			}
		}
	}

	async fn block_imported(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		match self.active_leaves.entry(block.hash) {
			hash_map::Entry::Vacant(entry) => entry.insert(block.number),
			hash_map::Entry::Occupied(entry) => {
				debug_assert_eq!(*entry.get(), block.number);
				return Ok(());
			}
		};

		let mut update = match self.on_head_activated(&block.hash, Some(block.parent_hash)) {
			Some((span, status)) => ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: block.hash,
				number: block.number,
				status,
				span
			}),
			None => ActiveLeavesUpdate::default(),
		};

		if let Some(number) = self.active_leaves.remove(&block.parent_hash) {
			debug_assert_eq!(block.number.saturating_sub(1), number);
			update.deactivated.push(block.parent_hash);
			self.on_head_deactivated(&block.parent_hash);
		}

		self.clean_up_external_listeners();

		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await
		} else {
			Ok(())
		}
	}

	async fn block_finalized(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		self.active_leaves.retain(|h, n| {
			if *n <= block.number {
				update.deactivated.push(*h);
				false
			} else {
				true
			}
		});

		for deactivated in &update.deactivated {
			self.on_head_deactivated(deactivated)
		}

		self.broadcast_signal(OverseerSignal::BlockFinalized(block.hash, block.number)).await?;

		// If there are no leaves being deactivated, we don't need to send an update.
		//
		// Our peers will be informed about our finalized block the next time we activating/deactivating some leaf.
		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
		}

		Ok(())
	}

	async fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
		self.subsystems.candidate_validation.send_signal(signal.clone()).await?;
		self.subsystems.candidate_backing.send_signal(signal.clone()).await?;
		self.subsystems.statement_distribution.send_signal(signal.clone()).await?;
		self.subsystems.availability_distribution.send_signal(signal.clone()).await?;
		self.subsystems.availability_recovery.send_signal(signal.clone()).await?;
		self.subsystems.bitfield_signing.send_signal(signal.clone()).await?;
		self.subsystems.bitfield_distribution.send_signal(signal.clone()).await?;
		self.subsystems.provisioner.send_signal(signal.clone()).await?;
		self.subsystems.runtime_api.send_signal(signal.clone()).await?;
		self.subsystems.availability_store.send_signal(signal.clone()).await?;
		self.subsystems.network_bridge.send_signal(signal.clone()).await?;
		self.subsystems.chain_api.send_signal(signal.clone()).await?;
		self.subsystems.collator_protocol.send_signal(signal.clone()).await?;
		self.subsystems.collation_generation.send_signal(signal.clone()).await?;
		self.subsystems.approval_distribution.send_signal(signal.clone()).await?;
		self.subsystems.approval_voting.send_signal(signal.clone()).await?;
		self.subsystems.gossip_support.send_signal(signal).await?;

		Ok(())
	}

	async fn route_message(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.metrics.on_message_relayed();
		match msg {
			AllMessages::CandidateValidation(msg) => {
				self.subsystems.candidate_validation.send_message(msg).await?;
			},
			AllMessages::CandidateBacking(msg) => {
				self.subsystems.candidate_backing.send_message(msg).await?;
			},
			AllMessages::StatementDistribution(msg) => {
				self.subsystems.statement_distribution.send_message(msg).await?;
			},
			AllMessages::AvailabilityDistribution(msg) => {
				self.subsystems.availability_distribution.send_message(msg).await?;
			},
			AllMessages::AvailabilityRecovery(msg) => {
				self.subsystems.availability_recovery.send_message(msg).await?;
			},
			AllMessages::BitfieldDistribution(msg) => {
				self.subsystems.bitfield_distribution.send_message(msg).await?;
			},
			AllMessages::BitfieldSigning(msg) => {
				self.subsystems.bitfield_signing.send_message(msg).await?;
			},
			AllMessages::Provisioner(msg) => {
				self.subsystems.provisioner.send_message(msg).await?;
			},
			AllMessages::RuntimeApi(msg) => {
				self.subsystems.runtime_api.send_message(msg).await?;
			},
			AllMessages::AvailabilityStore(msg) => {
				self.subsystems.availability_store.send_message(msg).await?;
			},
			AllMessages::NetworkBridge(msg) => {
				self.subsystems.network_bridge.send_message(msg).await?;
			},
			AllMessages::ChainApi(msg) => {
				self.subsystems.chain_api.send_message(msg).await?;
			},
			AllMessages::CollationGeneration(msg) => {
				self.subsystems.collation_generation.send_message(msg).await?;
			},
			AllMessages::CollatorProtocol(msg) => {
				self.subsystems.collator_protocol.send_message(msg).await?;
			},
			AllMessages::ApprovalDistribution(msg) => {
				self.subsystems.approval_distribution.send_message(msg).await?;
			},
			AllMessages::ApprovalVoting(msg) => {
				self.subsystems.approval_voting.send_message(msg).await?;
			},
			AllMessages::GossipSupport(msg) => {
				self.subsystems.gossip_support.send_message(msg).await?;
			},
			AllMessages::DisputeCoordinator(_) => {}
			AllMessages::DisputeParticipation(_) => {}
		}

		Ok(())
	}

	/// Handles a header activation. If the header's state doesn't support the parachains API,
	/// this returns `None`.
	fn on_head_activated(&mut self, hash: &Hash, parent_hash: Option<Hash>)
		-> Option<(Arc<jaeger::Span>, LeafStatus)>
	{
		if !self.supports_parachains.head_supports_parachains(hash) {
			return None;
		}

		self.metrics.on_head_activated();
		if let Some(listeners) = self.activation_external_listeners.remove(hash) {
			for listener in listeners {
				// it's fine if the listener is no longer interested
				let _ = listener.send(Ok(()));
			}
		}

		let mut span = jaeger::Span::new(*hash, "leaf-activated");

		if let Some(parent_span) = parent_hash.and_then(|h| self.span_per_active_leaf.get(&h)) {
			span.add_follows_from(&*parent_span);
		}

		let span = Arc::new(span);
		self.span_per_active_leaf.insert(*hash, span.clone());

		let status = if let Some(_) = self.known_leaves.put(*hash, ()) {
			LeafStatus::Stale
		} else {
			LeafStatus::Fresh
		};

		Some((span, status))
	}

	fn on_head_deactivated(&mut self, hash: &Hash) {
		self.metrics.on_head_deactivated();
		self.activation_external_listeners.remove(hash);
		self.span_per_active_leaf.remove(hash);
	}

	fn clean_up_external_listeners(&mut self) {
		self.activation_external_listeners.retain(|_, v| {
			// remove dead listeners
			v.retain(|c| !c.is_canceled());
			!v.is_empty()
		})
	}

	fn handle_external_request(&mut self, request: ExternalRequest) {
		match request {
			ExternalRequest::WaitForActivation { hash, response_channel } => {
				if self.active_leaves.get(&hash).is_some() {
					// it's fine if the listener is no longer interested
					let _ = response_channel.send(Ok(()));
				} else {
					self.activation_external_listeners.entry(hash).or_default().push(response_channel);
				}
			}
		}
	}

	fn spawn_job(&mut self, name: &'static str, j: BoxFuture<'static, ()>) {
		self.s.spawn(name, j);
	}

	fn spawn_blocking_job(&mut self, name: &'static str, j: BoxFuture<'static, ()>) {
		self.s.spawn_blocking(name, j);
	}
}

enum TaskKind {
	Regular,
	Blocking,
}

fn spawn<S: SpawnNamed, M: Send + 'static>(
	spawner: &mut S,
	message_tx: metered::MeteredSender<MessagePacket<M>>,
	message_rx: SubsystemIncomingMessages<M>,
	unbounded_meter: metered::Meter,
	to_subsystems: ChannelsOut,
	to_overseer_tx: metered::UnboundedMeteredSender<ToOverseer>,
	s: impl Subsystem<OverseerSubsystemContext<M>>,
	metrics: &Metrics,
	futures: &mut FuturesUnordered<BoxFuture<'static, SubsystemResult<()>>>,
	task_kind: TaskKind,
) -> SubsystemResult<OverseenSubsystem<M>> {
	let (signal_tx, signal_rx) = metered::channel(SIGNAL_CHANNEL_CAPACITY);
	let ctx = OverseerSubsystemContext::new(
		signal_rx,
		message_rx,
		to_subsystems,
		to_overseer_tx,
		metrics.clone(),
	);
	let SpawnedSubsystem { future, name } = s.start(ctx);

	let (tx, rx) = oneshot::channel();

	let fut = Box::pin(async move {
		if let Err(e) = future.await {
			tracing::error!(subsystem=name, err = ?e, "subsystem exited with error");
		} else {
			tracing::debug!(subsystem=name, "subsystem exited without an error");
		}
		let _ = tx.send(());
	});

	match task_kind {
		TaskKind::Regular => spawner.spawn(name, fut),
		TaskKind::Blocking => spawner.spawn_blocking(name, fut),
	}

	futures.push(Box::pin(rx.map(|e| { tracing::warn!(err = ?e, "dropping error"); Ok(()) })));

	let instance = Some(SubsystemInstance {
		meters: SubsystemMeters {
			unbounded: unbounded_meter,
			bounded: message_tx.meter().clone(),
			signals: signal_tx.meter().clone(),
		},
		tx_signal: signal_tx,
		tx_bounded: message_tx,
		signals_received: 0,
		name,
	});

	Ok(OverseenSubsystem {
		instance,
	})
}
