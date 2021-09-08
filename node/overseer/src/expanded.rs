// SystemTime { tv_sec: 1632404291, tv_nsec: 196243904 }
#[doc = r" Capacity of a bounded message channel between overseer and subsystem"]
#[doc = r" but also for bounded channels between two subsystems."]
const CHANNEL_CAPACITY: usize = 1024usize;
#[doc = r" Capacity of a signal channel between a subsystem and the overseer."]
const SIGNAL_CHANNEL_CAPACITY: usize = 64usize;
#[doc = r" The log target tag."]
const LOG_TARGET: &'static str = "overseer";
#[doc = r" The overseer."]
pub struct Overseer<S, SupportsParachains> {
	#[doc = r" A subsystem instance."]
	candidate_validation: OverseenSubsystem<CandidateValidationMessage>,
	#[doc = r" A subsystem instance."]
	candidate_backing: OverseenSubsystem<CandidateBackingMessage>,
	#[doc = r" A subsystem instance."]
	statement_distribution: OverseenSubsystem<StatementDistributionMessage>,
	#[doc = r" A subsystem instance."]
	availability_distribution: OverseenSubsystem<AvailabilityDistributionMessage>,
	#[doc = r" A subsystem instance."]
	availability_recovery: OverseenSubsystem<AvailabilityRecoveryMessage>,
	#[doc = r" A subsystem instance."]
	bitfield_signing: OverseenSubsystem<BitfieldSigningMessage>,
	#[doc = r" A subsystem instance."]
	bitfield_distribution: OverseenSubsystem<BitfieldDistributionMessage>,
	#[doc = r" A subsystem instance."]
	provisioner: OverseenSubsystem<ProvisionerMessage>,
	#[doc = r" A subsystem instance."]
	runtime_api: OverseenSubsystem<RuntimeApiMessage>,
	#[doc = r" A subsystem instance."]
	availability_store: OverseenSubsystem<AvailabilityStoreMessage>,
	#[doc = r" A subsystem instance."]
	network_bridge: OverseenSubsystem<NetworkBridgeMessage>,
	#[doc = r" A subsystem instance."]
	chain_api: OverseenSubsystem<ChainApiMessage>,
	#[doc = r" A subsystem instance."]
	collation_generation: OverseenSubsystem<CollationGenerationMessage>,
	#[doc = r" A subsystem instance."]
	collator_protocol: OverseenSubsystem<CollatorProtocolMessage>,
	#[doc = r" A subsystem instance."]
	approval_distribution: OverseenSubsystem<ApprovalDistributionMessage>,
	#[doc = r" A subsystem instance."]
	approval_voting: OverseenSubsystem<ApprovalVotingMessage>,
	#[doc = r" A subsystem instance."]
	gossip_support: OverseenSubsystem<GossipSupportMessage>,
	#[doc = r" A subsystem instance."]
	dispute_coordinator: OverseenSubsystem<DisputeCoordinatorMessage>,
	#[doc = r" A subsystem instance."]
	dispute_participation: OverseenSubsystem<DisputeParticipationMessage>,
	#[doc = r" A subsystem instance."]
	dispute_distribution: OverseenSubsystem<DisputeDistributionMessage>,
	#[doc = r" A subsystem instance."]
	chain_selection: OverseenSubsystem<ChainSelectionMessage>,
	#[doc = r" A user specified addendum field."]
	pub activation_external_listeners: HashMap<Hash, Vec<oneshot::Sender<SubsystemResult<()>>>>,
	#[doc = r" A user specified addendum field."]
	pub span_per_active_leaf: HashMap<Hash, Arc<jaeger::Span>>,
	#[doc = r" A user specified addendum field."]
	pub leaves: Vec<(Hash, BlockNumber)>,
	#[doc = r" A user specified addendum field."]
	pub active_leaves: HashMap<Hash, BlockNumber>,
	#[doc = r" A user specified addendum field."]
	pub supports_parachains: SupportsParachains,
	#[doc = r" A user specified addendum field."]
	pub known_leaves: LruCache<Hash, ()>,
	#[doc = r" A user specified addendum field."]
	pub metrics: OverseerMetrics,
	#[doc = r" Responsible for driving the subsystem futures."]
	spawner: S,
	#[doc = r" The set of running subsystems."]
	running_subsystems: polkadot_overseer_gen::FuturesUnordered<
		BoxFuture<'static, ::std::result::Result<(), SubsystemError>>,
	>,
	#[doc = r" Gather running subsystems' outbound streams into one."]
	to_overseer_rx: polkadot_overseer_gen::stream::Fuse<
		polkadot_overseer_gen::metered::UnboundedMeteredReceiver<polkadot_overseer_gen::ToOverseer>,
	>,
	#[doc = r" Events that are sent to the overseer from the outside world."]
	events_rx: polkadot_overseer_gen::metered::MeteredReceiver<Event>,
}
impl<S, SupportsParachains> Overseer<S, SupportsParachains>
where
	S: polkadot_overseer_gen::SpawnNamed,
{
	#[doc = r" Send the given signal, a termination signal, to all subsystems"]
	#[doc = r" and wait for all subsystems to go down."]
	#[doc = r""]
	#[doc = r" The definition of a termination signal is up to the user and"]
	#[doc = r" implementation specific."]
	pub async fn wait_terminate(
		&mut self,
		signal: OverseerSignal,
		timeout: ::std::time::Duration,
	) -> ::std::result::Result<(), SubsystemError> {
		::std::mem::drop(self.candidate_validation.send_signal(signal.clone()).await);
		::std::mem::drop(self.candidate_backing.send_signal(signal.clone()).await);
		::std::mem::drop(self.statement_distribution.send_signal(signal.clone()).await);
		::std::mem::drop(self.availability_distribution.send_signal(signal.clone()).await);
		::std::mem::drop(self.availability_recovery.send_signal(signal.clone()).await);
		::std::mem::drop(self.bitfield_signing.send_signal(signal.clone()).await);
		::std::mem::drop(self.bitfield_distribution.send_signal(signal.clone()).await);
		::std::mem::drop(self.provisioner.send_signal(signal.clone()).await);
		::std::mem::drop(self.runtime_api.send_signal(signal.clone()).await);
		::std::mem::drop(self.availability_store.send_signal(signal.clone()).await);
		::std::mem::drop(self.network_bridge.send_signal(signal.clone()).await);
		::std::mem::drop(self.chain_api.send_signal(signal.clone()).await);
		::std::mem::drop(self.collation_generation.send_signal(signal.clone()).await);
		::std::mem::drop(self.collator_protocol.send_signal(signal.clone()).await);
		::std::mem::drop(self.approval_distribution.send_signal(signal.clone()).await);
		::std::mem::drop(self.approval_voting.send_signal(signal.clone()).await);
		::std::mem::drop(self.gossip_support.send_signal(signal.clone()).await);
		::std::mem::drop(self.dispute_coordinator.send_signal(signal.clone()).await);
		::std::mem::drop(self.dispute_participation.send_signal(signal.clone()).await);
		::std::mem::drop(self.dispute_distribution.send_signal(signal.clone()).await);
		::std::mem::drop(self.chain_selection.send_signal(signal.clone()).await);
		let _ = signal;
		let mut timeout_fut = polkadot_overseer_gen::Delay::new(timeout).fuse();
		loop {
			select! {
				_ = self.running_subsystems.next() => if
				self.running_subsystems.is_empty() { break ; }, _ =
				timeout_fut => break, complete => break,
			}
		}
		Ok(())
	}
	#[doc = r" Broadcast a signal to all subsystems."]
	pub async fn broadcast_signal(
		&mut self,
		signal: OverseerSignal,
	) -> ::std::result::Result<(), SubsystemError> {
		let _ = self.candidate_validation.send_signal(signal.clone()).await;
		let _ = self.candidate_backing.send_signal(signal.clone()).await;
		let _ = self.statement_distribution.send_signal(signal.clone()).await;
		let _ = self.availability_distribution.send_signal(signal.clone()).await;
		let _ = self.availability_recovery.send_signal(signal.clone()).await;
		let _ = self.bitfield_signing.send_signal(signal.clone()).await;
		let _ = self.bitfield_distribution.send_signal(signal.clone()).await;
		let _ = self.provisioner.send_signal(signal.clone()).await;
		let _ = self.runtime_api.send_signal(signal.clone()).await;
		let _ = self.availability_store.send_signal(signal.clone()).await;
		let _ = self.network_bridge.send_signal(signal.clone()).await;
		let _ = self.chain_api.send_signal(signal.clone()).await;
		let _ = self.collation_generation.send_signal(signal.clone()).await;
		let _ = self.collator_protocol.send_signal(signal.clone()).await;
		let _ = self.approval_distribution.send_signal(signal.clone()).await;
		let _ = self.approval_voting.send_signal(signal.clone()).await;
		let _ = self.gossip_support.send_signal(signal.clone()).await;
		let _ = self.dispute_coordinator.send_signal(signal.clone()).await;
		let _ = self.dispute_participation.send_signal(signal.clone()).await;
		let _ = self.dispute_distribution.send_signal(signal.clone()).await;
		let _ = self.chain_selection.send_signal(signal.clone()).await;
		let _ = signal;
		Ok(())
	}
	#[doc = r" Route a particular message to a subsystem that consumes the message."]
	pub async fn route_message(
		&mut self,
		message: AllMessages,
		origin: &'static str,
	) -> ::std::result::Result<(), SubsystemError> {
		match message {
			AllMessages::CandidateValidation(inner) => {
				OverseenSubsystem::<CandidateValidationMessage>::send_message2(
					&mut self.candidate_validation,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::CandidateBacking(inner) => {
				OverseenSubsystem::<CandidateBackingMessage>::send_message2(
					&mut self.candidate_backing,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::StatementDistribution(inner) => {
				OverseenSubsystem::<StatementDistributionMessage>::send_message2(
					&mut self.statement_distribution,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::AvailabilityDistribution(inner) => {
				OverseenSubsystem::<AvailabilityDistributionMessage>::send_message2(
					&mut self.availability_distribution,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::AvailabilityRecovery(inner) => {
				OverseenSubsystem::<AvailabilityRecoveryMessage>::send_message2(
					&mut self.availability_recovery,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::BitfieldSigning(inner) => {
				OverseenSubsystem::<BitfieldSigningMessage>::send_message2(
					&mut self.bitfield_signing,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::BitfieldDistribution(inner) => {
				OverseenSubsystem::<BitfieldDistributionMessage>::send_message2(
					&mut self.bitfield_distribution,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::Provisioner(inner) => {
				OverseenSubsystem::<ProvisionerMessage>::send_message2(
					&mut self.provisioner,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::RuntimeApi(inner) => {
				OverseenSubsystem::<RuntimeApiMessage>::send_message2(
					&mut self.runtime_api,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::AvailabilityStore(inner) => {
				OverseenSubsystem::<AvailabilityStoreMessage>::send_message2(
					&mut self.availability_store,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::NetworkBridge(inner) => {
				OverseenSubsystem::<NetworkBridgeMessage>::send_message2(
					&mut self.network_bridge,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::ChainApi(inner) => {
				OverseenSubsystem::<ChainApiMessage>::send_message2(
					&mut self.chain_api,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::CollationGeneration(inner) => {
				OverseenSubsystem::<CollationGenerationMessage>::send_message2(
					&mut self.collation_generation,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::CollatorProtocol(inner) => {
				OverseenSubsystem::<CollatorProtocolMessage>::send_message2(
					&mut self.collator_protocol,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::ApprovalDistribution(inner) => {
				OverseenSubsystem::<ApprovalDistributionMessage>::send_message2(
					&mut self.approval_distribution,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::ApprovalVoting(inner) => {
				OverseenSubsystem::<ApprovalVotingMessage>::send_message2(
					&mut self.approval_voting,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::GossipSupport(inner) => {
				OverseenSubsystem::<GossipSupportMessage>::send_message2(
					&mut self.gossip_support,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::DisputeCoordinator(inner) => {
				OverseenSubsystem::<DisputeCoordinatorMessage>::send_message2(
					&mut self.dispute_coordinator,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::DisputeParticipation(inner) => {
				OverseenSubsystem::<DisputeParticipationMessage>::send_message2(
					&mut self.dispute_participation,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::DisputeDistribution(inner) => {
				OverseenSubsystem::<DisputeDistributionMessage>::send_message2(
					&mut self.dispute_distribution,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::ChainSelection(inner) => {
				OverseenSubsystem::<ChainSelectionMessage>::send_message2(
					&mut self.chain_selection,
					inner,
					origin,
				)
				.await?
			}
			AllMessages::Empty => {}
		}
		Ok(())
	}
	#[doc = r" Extract information from each subsystem."]
	pub fn map_subsystems<'a, Mapper, Output>(&'a self, mapper: Mapper) -> Vec<Output>
	where
		Mapper: MapSubsystem<&'a OverseenSubsystem<CandidateValidationMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<CandidateBackingMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<StatementDistributionMessage>, Output = Output>,
		Mapper:
			MapSubsystem<&'a OverseenSubsystem<AvailabilityDistributionMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<AvailabilityRecoveryMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<BitfieldSigningMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<BitfieldDistributionMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<ProvisionerMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<RuntimeApiMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<AvailabilityStoreMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<NetworkBridgeMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<ChainApiMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<CollationGenerationMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<CollatorProtocolMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<ApprovalDistributionMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<ApprovalVotingMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<GossipSupportMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<DisputeCoordinatorMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<DisputeParticipationMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<DisputeDistributionMessage>, Output = Output>,
		Mapper: MapSubsystem<&'a OverseenSubsystem<ChainSelectionMessage>, Output = Output>,
	{
		vec![
			mapper.map_subsystem(&self.candidate_validation),
			mapper.map_subsystem(&self.candidate_backing),
			mapper.map_subsystem(&self.statement_distribution),
			mapper.map_subsystem(&self.availability_distribution),
			mapper.map_subsystem(&self.availability_recovery),
			mapper.map_subsystem(&self.bitfield_signing),
			mapper.map_subsystem(&self.bitfield_distribution),
			mapper.map_subsystem(&self.provisioner),
			mapper.map_subsystem(&self.runtime_api),
			mapper.map_subsystem(&self.availability_store),
			mapper.map_subsystem(&self.network_bridge),
			mapper.map_subsystem(&self.chain_api),
			mapper.map_subsystem(&self.collation_generation),
			mapper.map_subsystem(&self.collator_protocol),
			mapper.map_subsystem(&self.approval_distribution),
			mapper.map_subsystem(&self.approval_voting),
			mapper.map_subsystem(&self.gossip_support),
			mapper.map_subsystem(&self.dispute_coordinator),
			mapper.map_subsystem(&self.dispute_participation),
			mapper.map_subsystem(&self.dispute_distribution),
			mapper.map_subsystem(&self.chain_selection),
		]
	}
	#[doc = r" Get access to internal task spawner."]
	pub fn spawner<'a>(&'a mut self) -> &'a mut S {
		&mut self.spawner
	}
}
impl<S, SupportsParachains> Overseer<S, SupportsParachains>
where
	S: polkadot_overseer_gen::SpawnNamed,
{
	#[doc = r" Create a new overseer utilizing the builder."]
	pub fn builder<
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>() -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		S: polkadot_overseer_gen::SpawnNamed,
		CandidateValidation:
			Subsystem<OverseerSubsystemContext<CandidateValidationMessage>, SubsystemError>,
		CandidateBacking:
			Subsystem<OverseerSubsystemContext<CandidateBackingMessage>, SubsystemError>,
		StatementDistribution:
			Subsystem<OverseerSubsystemContext<StatementDistributionMessage>, SubsystemError>,
		AvailabilityDistribution:
			Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>, SubsystemError>,
		AvailabilityRecovery:
			Subsystem<OverseerSubsystemContext<AvailabilityRecoveryMessage>, SubsystemError>,
		BitfieldSigning:
			Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>, SubsystemError>,
		BitfieldDistribution:
			Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>, SubsystemError>,
		Provisioner: Subsystem<OverseerSubsystemContext<ProvisionerMessage>, SubsystemError>,
		RuntimeApi: Subsystem<OverseerSubsystemContext<RuntimeApiMessage>, SubsystemError>,
		AvailabilityStore:
			Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>, SubsystemError>,
		NetworkBridge: Subsystem<OverseerSubsystemContext<NetworkBridgeMessage>, SubsystemError>,
		ChainApi: Subsystem<OverseerSubsystemContext<ChainApiMessage>, SubsystemError>,
		CollationGeneration:
			Subsystem<OverseerSubsystemContext<CollationGenerationMessage>, SubsystemError>,
		CollatorProtocol:
			Subsystem<OverseerSubsystemContext<CollatorProtocolMessage>, SubsystemError>,
		ApprovalDistribution:
			Subsystem<OverseerSubsystemContext<ApprovalDistributionMessage>, SubsystemError>,
		ApprovalVoting: Subsystem<OverseerSubsystemContext<ApprovalVotingMessage>, SubsystemError>,
		GossipSupport: Subsystem<OverseerSubsystemContext<GossipSupportMessage>, SubsystemError>,
		DisputeCoordinator:
			Subsystem<OverseerSubsystemContext<DisputeCoordinatorMessage>, SubsystemError>,
		DisputeParticipation:
			Subsystem<OverseerSubsystemContext<DisputeParticipationMessage>, SubsystemError>,
		DisputeDistribution:
			Subsystem<OverseerSubsystemContext<DisputeDistributionMessage>, SubsystemError>,
		ChainSelection: Subsystem<OverseerSubsystemContext<ChainSelectionMessage>, SubsystemError>,
	{
		OverseerBuilder::default()
	}
}
#[doc = r" Handle for an overseer."]
pub type OverseerHandle = polkadot_overseer_gen::metered::MeteredSender<Event>;
#[doc = r" External connector."]
pub struct OverseerConnector {
	#[doc = r" Publicly accessible handle, to be used for setting up"]
	#[doc = r" components that are _not_ subsystems but access is needed"]
	#[doc = r" due to other limitations."]
	#[doc = r""]
	#[doc = r" For subsystems, use the `_with` variants of the builder."]
	handle: OverseerHandle,
	#[doc = r" The side consumed by the `spawned` side of the overseer pattern."]
	consumer: polkadot_overseer_gen::metered::MeteredReceiver<Event>,
}
impl OverseerConnector {
	#[doc = r" Obtain access to the overseer handle."]
	pub fn as_handle_mut(&mut self) -> &mut OverseerHandle {
		&mut self.handle
	}
	#[doc = r" Obtain access to the overseer handle."]
	pub fn as_handle(&self) -> &OverseerHandle {
		&self.handle
	}
	#[doc = r" Obtain a clone of the handle."]
	pub fn handle(&self) -> OverseerHandle {
		self.handle.clone()
	}
}
impl ::std::default::Default for OverseerConnector {
	fn default() -> Self {
		let (events_tx, events_rx) =
			polkadot_overseer_gen::metered::channel::<Event>(SIGNAL_CHANNEL_CAPACITY);
		Self { handle: events_tx, consumer: events_rx }
	}
}
#[doc = r" Convenience alias."]
type SubsystemInitFn<T> =
	Box<dyn FnOnce(OverseerHandle) -> ::std::result::Result<T, SubsystemError>>;
#[doc = r" Initialization type to be used for a field of the overseer."]
enum FieldInitMethod<T> {
	#[doc = r" Defer initialization to a point where the `handle` is available."]
	Fn(SubsystemInitFn<T>),
	#[doc = r" Directly initialize the subsystem with the given subsystem type `T`."]
	Value(T),
	#[doc = r" Subsystem field does not have value just yet."]
	Uninitialized,
}
impl<T> ::std::default::Default for FieldInitMethod<T> {
	fn default() -> Self {
		Self::Uninitialized
	}
}
#[allow(missing_docs)]
pub struct OverseerBuilder<
	S,
	SupportsParachains,
	CandidateValidation,
	CandidateBacking,
	StatementDistribution,
	AvailabilityDistribution,
	AvailabilityRecovery,
	BitfieldSigning,
	BitfieldDistribution,
	Provisioner,
	RuntimeApi,
	AvailabilityStore,
	NetworkBridge,
	ChainApi,
	CollationGeneration,
	CollatorProtocol,
	ApprovalDistribution,
	ApprovalVoting,
	GossipSupport,
	DisputeCoordinator,
	DisputeParticipation,
	DisputeDistribution,
	ChainSelection,
> {
	candidate_validation: FieldInitMethod<CandidateValidation>,
	candidate_backing: FieldInitMethod<CandidateBacking>,
	statement_distribution: FieldInitMethod<StatementDistribution>,
	availability_distribution: FieldInitMethod<AvailabilityDistribution>,
	availability_recovery: FieldInitMethod<AvailabilityRecovery>,
	bitfield_signing: FieldInitMethod<BitfieldSigning>,
	bitfield_distribution: FieldInitMethod<BitfieldDistribution>,
	provisioner: FieldInitMethod<Provisioner>,
	runtime_api: FieldInitMethod<RuntimeApi>,
	availability_store: FieldInitMethod<AvailabilityStore>,
	network_bridge: FieldInitMethod<NetworkBridge>,
	chain_api: FieldInitMethod<ChainApi>,
	collation_generation: FieldInitMethod<CollationGeneration>,
	collator_protocol: FieldInitMethod<CollatorProtocol>,
	approval_distribution: FieldInitMethod<ApprovalDistribution>,
	approval_voting: FieldInitMethod<ApprovalVoting>,
	gossip_support: FieldInitMethod<GossipSupport>,
	dispute_coordinator: FieldInitMethod<DisputeCoordinator>,
	dispute_participation: FieldInitMethod<DisputeParticipation>,
	dispute_distribution: FieldInitMethod<DisputeDistribution>,
	chain_selection: FieldInitMethod<ChainSelection>,
	activation_external_listeners:
		::std::option::Option<HashMap<Hash, Vec<oneshot::Sender<SubsystemResult<()>>>>>,
	span_per_active_leaf: ::std::option::Option<HashMap<Hash, Arc<jaeger::Span>>>,
	leaves: ::std::option::Option<Vec<(Hash, BlockNumber)>>,
	active_leaves: ::std::option::Option<HashMap<Hash, BlockNumber>>,
	supports_parachains: ::std::option::Option<SupportsParachains>,
	known_leaves: ::std::option::Option<LruCache<Hash, ()>>,
	metrics: ::std::option::Option<OverseerMetrics>,
	spawner: ::std::option::Option<S>,
}
impl<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	> Default
	for OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
{
	fn default() -> Self {
		fn trait_from_must_be_implemented<E>()
		where
			E: std::error::Error
				+ Send
				+ Sync
				+ 'static
				+ From<polkadot_overseer_gen::OverseerError>,
		{
		}
		trait_from_must_be_implemented::<SubsystemError>();
		Self {
			candidate_validation: Default::default(),
			candidate_backing: Default::default(),
			statement_distribution: Default::default(),
			availability_distribution: Default::default(),
			availability_recovery: Default::default(),
			bitfield_signing: Default::default(),
			bitfield_distribution: Default::default(),
			provisioner: Default::default(),
			runtime_api: Default::default(),
			availability_store: Default::default(),
			network_bridge: Default::default(),
			chain_api: Default::default(),
			collation_generation: Default::default(),
			collator_protocol: Default::default(),
			approval_distribution: Default::default(),
			approval_voting: Default::default(),
			gossip_support: Default::default(),
			dispute_coordinator: Default::default(),
			dispute_participation: Default::default(),
			dispute_distribution: Default::default(),
			chain_selection: Default::default(),
			activation_external_listeners: None,
			span_per_active_leaf: None,
			leaves: None,
			active_leaves: None,
			supports_parachains: None,
			known_leaves: None,
			metrics: None,
			spawner: None,
		}
	}
}
impl<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	> where
	S: polkadot_overseer_gen::SpawnNamed,
	CandidateValidation:
		Subsystem<OverseerSubsystemContext<CandidateValidationMessage>, SubsystemError>,
	CandidateBacking: Subsystem<OverseerSubsystemContext<CandidateBackingMessage>, SubsystemError>,
	StatementDistribution:
		Subsystem<OverseerSubsystemContext<StatementDistributionMessage>, SubsystemError>,
	AvailabilityDistribution:
		Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>, SubsystemError>,
	AvailabilityRecovery:
		Subsystem<OverseerSubsystemContext<AvailabilityRecoveryMessage>, SubsystemError>,
	BitfieldSigning: Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>, SubsystemError>,
	BitfieldDistribution:
		Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>, SubsystemError>,
	Provisioner: Subsystem<OverseerSubsystemContext<ProvisionerMessage>, SubsystemError>,
	RuntimeApi: Subsystem<OverseerSubsystemContext<RuntimeApiMessage>, SubsystemError>,
	AvailabilityStore:
		Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>, SubsystemError>,
	NetworkBridge: Subsystem<OverseerSubsystemContext<NetworkBridgeMessage>, SubsystemError>,
	ChainApi: Subsystem<OverseerSubsystemContext<ChainApiMessage>, SubsystemError>,
	CollationGeneration:
		Subsystem<OverseerSubsystemContext<CollationGenerationMessage>, SubsystemError>,
	CollatorProtocol: Subsystem<OverseerSubsystemContext<CollatorProtocolMessage>, SubsystemError>,
	ApprovalDistribution:
		Subsystem<OverseerSubsystemContext<ApprovalDistributionMessage>, SubsystemError>,
	ApprovalVoting: Subsystem<OverseerSubsystemContext<ApprovalVotingMessage>, SubsystemError>,
	GossipSupport: Subsystem<OverseerSubsystemContext<GossipSupportMessage>, SubsystemError>,
	DisputeCoordinator:
		Subsystem<OverseerSubsystemContext<DisputeCoordinatorMessage>, SubsystemError>,
	DisputeParticipation:
		Subsystem<OverseerSubsystemContext<DisputeParticipationMessage>, SubsystemError>,
	DisputeDistribution:
		Subsystem<OverseerSubsystemContext<DisputeDistributionMessage>, SubsystemError>,
	ChainSelection: Subsystem<OverseerSubsystemContext<ChainSelectionMessage>, SubsystemError>,
{
	#[doc = r" The spawner to use for spawning tasks."]
	pub fn spawner(mut self, spawner: S) -> Self
	where
		S: polkadot_overseer_gen::SpawnNamed + Send,
	{
		self.spawner = Some(spawner);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn candidate_validation(mut self, subsystem: CandidateValidation) -> Self {
		self.candidate_validation = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn candidate_validation_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<CandidateValidation, SubsystemError>,
	{
		self.candidate_validation = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<CandidateValidation>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn candidate_backing(mut self, subsystem: CandidateBacking) -> Self {
		self.candidate_backing = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn candidate_backing_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<CandidateBacking, SubsystemError>,
	{
		self.candidate_backing =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<CandidateBacking>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn statement_distribution(mut self, subsystem: StatementDistribution) -> Self {
		self.statement_distribution = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn statement_distribution_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<StatementDistribution, SubsystemError>,
	{
		self.statement_distribution = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<StatementDistribution>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn availability_distribution(mut self, subsystem: AvailabilityDistribution) -> Self {
		self.availability_distribution = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn availability_distribution_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(
				OverseerHandle,
			) -> ::std::result::Result<AvailabilityDistribution, SubsystemError>,
	{
		self.availability_distribution = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<AvailabilityDistribution>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn availability_recovery(mut self, subsystem: AvailabilityRecovery) -> Self {
		self.availability_recovery = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn availability_recovery_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<AvailabilityRecovery, SubsystemError>,
	{
		self.availability_recovery = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<AvailabilityRecovery>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn bitfield_signing(mut self, subsystem: BitfieldSigning) -> Self {
		self.bitfield_signing = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn bitfield_signing_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<BitfieldSigning, SubsystemError>,
	{
		self.bitfield_signing =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<BitfieldSigning>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn bitfield_distribution(mut self, subsystem: BitfieldDistribution) -> Self {
		self.bitfield_distribution = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn bitfield_distribution_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<BitfieldDistribution, SubsystemError>,
	{
		self.bitfield_distribution = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<BitfieldDistribution>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn provisioner(mut self, subsystem: Provisioner) -> Self {
		self.provisioner = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn provisioner_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static + FnOnce(OverseerHandle) -> ::std::result::Result<Provisioner, SubsystemError>,
	{
		self.provisioner =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<Provisioner>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn runtime_api(mut self, subsystem: RuntimeApi) -> Self {
		self.runtime_api = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn runtime_api_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static + FnOnce(OverseerHandle) -> ::std::result::Result<RuntimeApi, SubsystemError>,
	{
		self.runtime_api =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<RuntimeApi>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn availability_store(mut self, subsystem: AvailabilityStore) -> Self {
		self.availability_store = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn availability_store_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<AvailabilityStore, SubsystemError>,
	{
		self.availability_store =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<AvailabilityStore>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn network_bridge(mut self, subsystem: NetworkBridge) -> Self {
		self.network_bridge = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn network_bridge_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static + FnOnce(OverseerHandle) -> ::std::result::Result<NetworkBridge, SubsystemError>,
	{
		self.network_bridge =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<NetworkBridge>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn chain_api(mut self, subsystem: ChainApi) -> Self {
		self.chain_api = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn chain_api_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static + FnOnce(OverseerHandle) -> ::std::result::Result<ChainApi, SubsystemError>,
	{
		self.chain_api =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<ChainApi>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn collation_generation(mut self, subsystem: CollationGeneration) -> Self {
		self.collation_generation = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn collation_generation_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<CollationGeneration, SubsystemError>,
	{
		self.collation_generation = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<CollationGeneration>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn collator_protocol(mut self, subsystem: CollatorProtocol) -> Self {
		self.collator_protocol = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn collator_protocol_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<CollatorProtocol, SubsystemError>,
	{
		self.collator_protocol =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<CollatorProtocol>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn approval_distribution(mut self, subsystem: ApprovalDistribution) -> Self {
		self.approval_distribution = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn approval_distribution_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<ApprovalDistribution, SubsystemError>,
	{
		self.approval_distribution = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<ApprovalDistribution>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn approval_voting(mut self, subsystem: ApprovalVoting) -> Self {
		self.approval_voting = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn approval_voting_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<ApprovalVoting, SubsystemError>,
	{
		self.approval_voting =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<ApprovalVoting>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn gossip_support(mut self, subsystem: GossipSupport) -> Self {
		self.gossip_support = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn gossip_support_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static + FnOnce(OverseerHandle) -> ::std::result::Result<GossipSupport, SubsystemError>,
	{
		self.gossip_support =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<GossipSupport>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn dispute_coordinator(mut self, subsystem: DisputeCoordinator) -> Self {
		self.dispute_coordinator = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn dispute_coordinator_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<DisputeCoordinator, SubsystemError>,
	{
		self.dispute_coordinator =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<DisputeCoordinator>);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn dispute_participation(mut self, subsystem: DisputeParticipation) -> Self {
		self.dispute_participation = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn dispute_participation_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<DisputeParticipation, SubsystemError>,
	{
		self.dispute_participation = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<DisputeParticipation>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn dispute_distribution(mut self, subsystem: DisputeDistribution) -> Self {
		self.dispute_distribution = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn dispute_distribution_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<DisputeDistribution, SubsystemError>,
	{
		self.dispute_distribution = FieldInitMethod::Fn(
			Box::new(subsystem_init_fn) as SubsystemInitFn<DisputeDistribution>
		);
		self
	}
	#[doc = r" Specify the particular subsystem implementation."]
	pub fn chain_selection(mut self, subsystem: ChainSelection) -> Self {
		self.chain_selection = FieldInitMethod::Value(subsystem);
		self
	}
	#[doc = r" Specify the particular subsystem by giving a init function."]
	pub fn chain_selection_with<'a, F>(mut self, subsystem_init_fn: F) -> Self
	where
		F: 'static
			+ FnOnce(OverseerHandle) -> ::std::result::Result<ChainSelection, SubsystemError>,
	{
		self.chain_selection =
			FieldInitMethod::Fn(Box::new(subsystem_init_fn) as SubsystemInitFn<ChainSelection>);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn activation_external_listeners(
		mut self,
		baggage: HashMap<Hash, Vec<oneshot::Sender<SubsystemResult<()>>>>,
	) -> Self {
		self.activation_external_listeners = Some(baggage);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn span_per_active_leaf(mut self, baggage: HashMap<Hash, Arc<jaeger::Span>>) -> Self {
		self.span_per_active_leaf = Some(baggage);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn leaves(mut self, baggage: Vec<(Hash, BlockNumber)>) -> Self {
		self.leaves = Some(baggage);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn active_leaves(mut self, baggage: HashMap<Hash, BlockNumber>) -> Self {
		self.active_leaves = Some(baggage);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn supports_parachains(mut self, baggage: SupportsParachains) -> Self {
		self.supports_parachains = Some(baggage);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn known_leaves(mut self, baggage: LruCache<Hash, ()>) -> Self {
		self.known_leaves = Some(baggage);
		self
	}
	#[doc = r" Attach the user defined addendum type."]
	pub fn metrics(mut self, baggage: OverseerMetrics) -> Self {
		self.metrics = Some(baggage);
		self
	}
	#[doc = r" Complete the construction and create the overseer type."]
	pub fn build(
		self,
	) -> ::std::result::Result<(Overseer<S, SupportsParachains>, OverseerHandle), SubsystemError> {
		let connector = OverseerConnector::default();
		self.build_with_connector(connector)
	}
	#[doc = r" Complete the construction and create the overseer type based on an existing `connector`."]
	pub fn build_with_connector(
		self,
		connector: OverseerConnector,
	) -> ::std::result::Result<(Overseer<S, SupportsParachains>, OverseerHandle), SubsystemError> {
		let OverseerConnector { handle: events_tx, consumer: events_rx } = connector;
		let handle = events_tx.clone();
		let (to_overseer_tx, to_overseer_rx) =
			polkadot_overseer_gen::metered::unbounded::<ToOverseer>();
		let (candidate_validation_tx, candidate_validation_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<CandidateValidationMessage>>(
				CHANNEL_CAPACITY,
			);
		let (candidate_backing_tx, candidate_backing_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<CandidateBackingMessage>,
		>(CHANNEL_CAPACITY);
		let (statement_distribution_tx, statement_distribution_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<StatementDistributionMessage>>(
				CHANNEL_CAPACITY,
			);
		let (availability_distribution_tx, availability_distribution_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<AvailabilityDistributionMessage>>(
				CHANNEL_CAPACITY,
			);
		let (availability_recovery_tx, availability_recovery_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<AvailabilityRecoveryMessage>>(
				CHANNEL_CAPACITY,
			);
		let (bitfield_signing_tx, bitfield_signing_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<BitfieldSigningMessage>,
		>(CHANNEL_CAPACITY);
		let (bitfield_distribution_tx, bitfield_distribution_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<BitfieldDistributionMessage>>(
				CHANNEL_CAPACITY,
			);
		let (provisioner_tx, provisioner_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<ProvisionerMessage>,
		>(CHANNEL_CAPACITY);
		let (runtime_api_tx, runtime_api_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<RuntimeApiMessage>,
		>(CHANNEL_CAPACITY);
		let (availability_store_tx, availability_store_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<AvailabilityStoreMessage>>(
				CHANNEL_CAPACITY,
			);
		let (network_bridge_tx, network_bridge_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<NetworkBridgeMessage>,
		>(CHANNEL_CAPACITY);
		let (chain_api_tx, chain_api_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<ChainApiMessage>,
		>(CHANNEL_CAPACITY);
		let (collation_generation_tx, collation_generation_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<CollationGenerationMessage>>(
				CHANNEL_CAPACITY,
			);
		let (collator_protocol_tx, collator_protocol_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<CollatorProtocolMessage>,
		>(CHANNEL_CAPACITY);
		let (approval_distribution_tx, approval_distribution_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<ApprovalDistributionMessage>>(
				CHANNEL_CAPACITY,
			);
		let (approval_voting_tx, approval_voting_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<ApprovalVotingMessage>,
		>(CHANNEL_CAPACITY);
		let (gossip_support_tx, gossip_support_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<GossipSupportMessage>,
		>(CHANNEL_CAPACITY);
		let (dispute_coordinator_tx, dispute_coordinator_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<DisputeCoordinatorMessage>>(
				CHANNEL_CAPACITY,
			);
		let (dispute_participation_tx, dispute_participation_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<DisputeParticipationMessage>>(
				CHANNEL_CAPACITY,
			);
		let (dispute_distribution_tx, dispute_distribution_rx) =
			polkadot_overseer_gen::metered::channel::<MessagePacket<DisputeDistributionMessage>>(
				CHANNEL_CAPACITY,
			);
		let (chain_selection_tx, chain_selection_rx) = polkadot_overseer_gen::metered::channel::<
			MessagePacket<ChainSelectionMessage>,
		>(CHANNEL_CAPACITY);
		let (candidate_validation_unbounded_tx, candidate_validation_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<CandidateValidationMessage>>(
			);
		let (candidate_backing_unbounded_tx, candidate_backing_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<CandidateBackingMessage>>();
		let (statement_distribution_unbounded_tx, statement_distribution_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<StatementDistributionMessage>>(
			);
		let (availability_distribution_unbounded_tx, availability_distribution_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<
				MessagePacket<AvailabilityDistributionMessage>,
			>();
		let (availability_recovery_unbounded_tx, availability_recovery_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<AvailabilityRecoveryMessage>>(
			);
		let (bitfield_signing_unbounded_tx, bitfield_signing_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<BitfieldSigningMessage>>();
		let (bitfield_distribution_unbounded_tx, bitfield_distribution_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<BitfieldDistributionMessage>>(
			);
		let (provisioner_unbounded_tx, provisioner_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<ProvisionerMessage>>();
		let (runtime_api_unbounded_tx, runtime_api_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<RuntimeApiMessage>>();
		let (availability_store_unbounded_tx, availability_store_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<AvailabilityStoreMessage>>();
		let (network_bridge_unbounded_tx, network_bridge_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<NetworkBridgeMessage>>();
		let (chain_api_unbounded_tx, chain_api_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<ChainApiMessage>>();
		let (collation_generation_unbounded_tx, collation_generation_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<CollationGenerationMessage>>(
			);
		let (collator_protocol_unbounded_tx, collator_protocol_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<CollatorProtocolMessage>>();
		let (approval_distribution_unbounded_tx, approval_distribution_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<ApprovalDistributionMessage>>(
			);
		let (approval_voting_unbounded_tx, approval_voting_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<ApprovalVotingMessage>>();
		let (gossip_support_unbounded_tx, gossip_support_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<GossipSupportMessage>>();
		let (dispute_coordinator_unbounded_tx, dispute_coordinator_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<DisputeCoordinatorMessage>>();
		let (dispute_participation_unbounded_tx, dispute_participation_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<DisputeParticipationMessage>>(
			);
		let (dispute_distribution_unbounded_tx, dispute_distribution_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<DisputeDistributionMessage>>(
			);
		let (chain_selection_unbounded_tx, chain_selection_unbounded_rx) =
			polkadot_overseer_gen::metered::unbounded::<MessagePacket<ChainSelectionMessage>>();
		let channels_out = ChannelsOut {
			candidate_validation: candidate_validation_tx.clone(),
			candidate_backing: candidate_backing_tx.clone(),
			statement_distribution: statement_distribution_tx.clone(),
			availability_distribution: availability_distribution_tx.clone(),
			availability_recovery: availability_recovery_tx.clone(),
			bitfield_signing: bitfield_signing_tx.clone(),
			bitfield_distribution: bitfield_distribution_tx.clone(),
			provisioner: provisioner_tx.clone(),
			runtime_api: runtime_api_tx.clone(),
			availability_store: availability_store_tx.clone(),
			network_bridge: network_bridge_tx.clone(),
			chain_api: chain_api_tx.clone(),
			collation_generation: collation_generation_tx.clone(),
			collator_protocol: collator_protocol_tx.clone(),
			approval_distribution: approval_distribution_tx.clone(),
			approval_voting: approval_voting_tx.clone(),
			gossip_support: gossip_support_tx.clone(),
			dispute_coordinator: dispute_coordinator_tx.clone(),
			dispute_participation: dispute_participation_tx.clone(),
			dispute_distribution: dispute_distribution_tx.clone(),
			chain_selection: chain_selection_tx.clone(),
			candidate_validation_unbounded: candidate_validation_unbounded_tx,
			candidate_backing_unbounded: candidate_backing_unbounded_tx,
			statement_distribution_unbounded: statement_distribution_unbounded_tx,
			availability_distribution_unbounded: availability_distribution_unbounded_tx,
			availability_recovery_unbounded: availability_recovery_unbounded_tx,
			bitfield_signing_unbounded: bitfield_signing_unbounded_tx,
			bitfield_distribution_unbounded: bitfield_distribution_unbounded_tx,
			provisioner_unbounded: provisioner_unbounded_tx,
			runtime_api_unbounded: runtime_api_unbounded_tx,
			availability_store_unbounded: availability_store_unbounded_tx,
			network_bridge_unbounded: network_bridge_unbounded_tx,
			chain_api_unbounded: chain_api_unbounded_tx,
			collation_generation_unbounded: collation_generation_unbounded_tx,
			collator_protocol_unbounded: collator_protocol_unbounded_tx,
			approval_distribution_unbounded: approval_distribution_unbounded_tx,
			approval_voting_unbounded: approval_voting_unbounded_tx,
			gossip_support_unbounded: gossip_support_unbounded_tx,
			dispute_coordinator_unbounded: dispute_coordinator_unbounded_tx,
			dispute_participation_unbounded: dispute_participation_unbounded_tx,
			dispute_distribution_unbounded: dispute_distribution_unbounded_tx,
			chain_selection_unbounded: chain_selection_unbounded_tx,
		};
		let mut spawner = self.spawner.expect("Spawner is set. qed");
		let mut running_subsystems = polkadot_overseer_gen::FuturesUnordered::<
			BoxFuture<'static, ::std::result::Result<(), SubsystemError>>,
		>::new();
		let candidate_validation = match self.candidate_validation {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = candidate_validation_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<CandidateValidationMessage> =
			polkadot_overseer_gen::select(
				candidate_validation_rx,
				candidate_validation_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<CandidateValidationMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let candidate_validation: OverseenSubsystem<CandidateValidationMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				candidate_validation_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				candidate_validation,
				&mut running_subsystems,
			)?;
		let candidate_backing = match self.candidate_backing {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = candidate_backing_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<CandidateBackingMessage> =
			polkadot_overseer_gen::select(candidate_backing_rx, candidate_backing_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<CandidateBackingMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let candidate_backing: OverseenSubsystem<CandidateBackingMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				candidate_backing_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				candidate_backing,
				&mut running_subsystems,
			)?;
		let statement_distribution = match self.statement_distribution {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = statement_distribution_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<StatementDistributionMessage> =
			polkadot_overseer_gen::select(
				statement_distribution_rx,
				statement_distribution_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<StatementDistributionMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let statement_distribution: OverseenSubsystem<StatementDistributionMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				statement_distribution_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				statement_distribution,
				&mut running_subsystems,
			)?;
		let availability_distribution = match self.availability_distribution {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = availability_distribution_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<AvailabilityDistributionMessage> =
			polkadot_overseer_gen::select(
				availability_distribution_rx,
				availability_distribution_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<AvailabilityDistributionMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let availability_distribution: OverseenSubsystem<AvailabilityDistributionMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				availability_distribution_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				availability_distribution,
				&mut running_subsystems,
			)?;
		let availability_recovery = match self.availability_recovery {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = availability_recovery_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<AvailabilityRecoveryMessage> =
			polkadot_overseer_gen::select(
				availability_recovery_rx,
				availability_recovery_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<AvailabilityRecoveryMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let availability_recovery: OverseenSubsystem<AvailabilityRecoveryMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				availability_recovery_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				availability_recovery,
				&mut running_subsystems,
			)?;
		let bitfield_signing = match self.bitfield_signing {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = bitfield_signing_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<BitfieldSigningMessage> =
			polkadot_overseer_gen::select(bitfield_signing_rx, bitfield_signing_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<BitfieldSigningMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let bitfield_signing: OverseenSubsystem<BitfieldSigningMessage> =
			spawn::<_, _, Blocking, _, _, _>(
				&mut spawner,
				bitfield_signing_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				bitfield_signing,
				&mut running_subsystems,
			)?;
		let bitfield_distribution = match self.bitfield_distribution {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = bitfield_distribution_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<BitfieldDistributionMessage> =
			polkadot_overseer_gen::select(
				bitfield_distribution_rx,
				bitfield_distribution_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<BitfieldDistributionMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let bitfield_distribution: OverseenSubsystem<BitfieldDistributionMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				bitfield_distribution_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				bitfield_distribution,
				&mut running_subsystems,
			)?;
		let provisioner = match self.provisioner {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = provisioner_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<ProvisionerMessage> =
			polkadot_overseer_gen::select(provisioner_rx, provisioner_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<ProvisionerMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let provisioner: OverseenSubsystem<ProvisionerMessage> = spawn::<_, _, Regular, _, _, _>(
			&mut spawner,
			provisioner_tx,
			signal_tx,
			unbounded_meter,
			ctx,
			provisioner,
			&mut running_subsystems,
		)?;
		let runtime_api = match self.runtime_api {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = runtime_api_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<RuntimeApiMessage> =
			polkadot_overseer_gen::select(runtime_api_rx, runtime_api_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<RuntimeApiMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let runtime_api: OverseenSubsystem<RuntimeApiMessage> = spawn::<_, _, Blocking, _, _, _>(
			&mut spawner,
			runtime_api_tx,
			signal_tx,
			unbounded_meter,
			ctx,
			runtime_api,
			&mut running_subsystems,
		)?;
		let availability_store = match self.availability_store {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = availability_store_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<AvailabilityStoreMessage> =
			polkadot_overseer_gen::select(availability_store_rx, availability_store_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<AvailabilityStoreMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let availability_store: OverseenSubsystem<AvailabilityStoreMessage> =
			spawn::<_, _, Blocking, _, _, _>(
				&mut spawner,
				availability_store_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				availability_store,
				&mut running_subsystems,
			)?;
		let network_bridge = match self.network_bridge {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = network_bridge_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<NetworkBridgeMessage> =
			polkadot_overseer_gen::select(network_bridge_rx, network_bridge_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<NetworkBridgeMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let network_bridge: OverseenSubsystem<NetworkBridgeMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				network_bridge_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				network_bridge,
				&mut running_subsystems,
			)?;
		let chain_api = match self.chain_api {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = chain_api_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<ChainApiMessage> =
			polkadot_overseer_gen::select(chain_api_rx, chain_api_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<ChainApiMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let chain_api: OverseenSubsystem<ChainApiMessage> = spawn::<_, _, Blocking, _, _, _>(
			&mut spawner,
			chain_api_tx,
			signal_tx,
			unbounded_meter,
			ctx,
			chain_api,
			&mut running_subsystems,
		)?;
		let collation_generation = match self.collation_generation {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = collation_generation_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<CollationGenerationMessage> =
			polkadot_overseer_gen::select(
				collation_generation_rx,
				collation_generation_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<CollationGenerationMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let collation_generation: OverseenSubsystem<CollationGenerationMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				collation_generation_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				collation_generation,
				&mut running_subsystems,
			)?;
		let collator_protocol = match self.collator_protocol {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = collator_protocol_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<CollatorProtocolMessage> =
			polkadot_overseer_gen::select(collator_protocol_rx, collator_protocol_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<CollatorProtocolMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let collator_protocol: OverseenSubsystem<CollatorProtocolMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				collator_protocol_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				collator_protocol,
				&mut running_subsystems,
			)?;
		let approval_distribution = match self.approval_distribution {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = approval_distribution_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<ApprovalDistributionMessage> =
			polkadot_overseer_gen::select(
				approval_distribution_rx,
				approval_distribution_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<ApprovalDistributionMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let approval_distribution: OverseenSubsystem<ApprovalDistributionMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				approval_distribution_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				approval_distribution,
				&mut running_subsystems,
			)?;
		let approval_voting = match self.approval_voting {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = approval_voting_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<ApprovalVotingMessage> =
			polkadot_overseer_gen::select(approval_voting_rx, approval_voting_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<ApprovalVotingMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let approval_voting: OverseenSubsystem<ApprovalVotingMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				approval_voting_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				approval_voting,
				&mut running_subsystems,
			)?;
		let gossip_support = match self.gossip_support {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = gossip_support_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<GossipSupportMessage> =
			polkadot_overseer_gen::select(gossip_support_rx, gossip_support_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<GossipSupportMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let gossip_support: OverseenSubsystem<GossipSupportMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				gossip_support_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				gossip_support,
				&mut running_subsystems,
			)?;
		let dispute_coordinator = match self.dispute_coordinator {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = dispute_coordinator_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<DisputeCoordinatorMessage> =
			polkadot_overseer_gen::select(dispute_coordinator_rx, dispute_coordinator_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<DisputeCoordinatorMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let dispute_coordinator: OverseenSubsystem<DisputeCoordinatorMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				dispute_coordinator_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				dispute_coordinator,
				&mut running_subsystems,
			)?;
		let dispute_participation = match self.dispute_participation {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = dispute_participation_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<DisputeParticipationMessage> =
			polkadot_overseer_gen::select(
				dispute_participation_rx,
				dispute_participation_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<DisputeParticipationMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let dispute_participation: OverseenSubsystem<DisputeParticipationMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				dispute_participation_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				dispute_participation,
				&mut running_subsystems,
			)?;
		let dispute_distribution = match self.dispute_distribution {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = dispute_distribution_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<DisputeDistributionMessage> =
			polkadot_overseer_gen::select(
				dispute_distribution_rx,
				dispute_distribution_unbounded_rx,
			);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<DisputeDistributionMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let dispute_distribution: OverseenSubsystem<DisputeDistributionMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				dispute_distribution_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				dispute_distribution,
				&mut running_subsystems,
			)?;
		let chain_selection = match self.chain_selection {
			FieldInitMethod::Fn(func) => func(handle.clone())?,
			FieldInitMethod::Value(val) => val,
			FieldInitMethod::Uninitialized => {
				panic!("All subsystems must exist with the builder pattern.")
			}
		};
		let unbounded_meter = chain_selection_unbounded_rx.meter().clone();
		let message_rx: SubsystemIncomingMessages<ChainSelectionMessage> =
			polkadot_overseer_gen::select(chain_selection_rx, chain_selection_unbounded_rx);
		let (signal_tx, signal_rx) =
			polkadot_overseer_gen::metered::channel(SIGNAL_CHANNEL_CAPACITY);
		let ctx = OverseerSubsystemContext::<ChainSelectionMessage>::new(
			signal_rx,
			message_rx,
			channels_out.clone(),
			to_overseer_tx.clone(),
		);
		let chain_selection: OverseenSubsystem<ChainSelectionMessage> =
			spawn::<_, _, Regular, _, _, _>(
				&mut spawner,
				chain_selection_tx,
				signal_tx,
				unbounded_meter,
				ctx,
				chain_selection,
				&mut running_subsystems,
			)?;
		let activation_external_listeners = self.activation_external_listeners.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(activation_external_listeners)
		));
		let span_per_active_leaf = self.span_per_active_leaf.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(span_per_active_leaf)
		));
		let leaves = self.leaves.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(leaves)
		));
		let active_leaves = self.active_leaves.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(active_leaves)
		));
		let supports_parachains = self.supports_parachains.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(supports_parachains)
		));
		let known_leaves = self.known_leaves.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(known_leaves)
		));
		let metrics = self.metrics.expect(&format!(
			"Baggage variable `{1}` of `{0}` ",
			stringify!(Overseer),
			stringify!(metrics)
		));
		use polkadot_overseer_gen::StreamExt;
		let to_overseer_rx = to_overseer_rx.fuse();
		let overseer = Overseer {
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
			running_subsystems,
			events_rx,
			to_overseer_rx,
		};
		Ok((overseer, handle))
	}
}
impl<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	> where
	S: polkadot_overseer_gen::SpawnNamed,
	CandidateValidation:
		Subsystem<OverseerSubsystemContext<CandidateValidationMessage>, SubsystemError>,
	CandidateBacking: Subsystem<OverseerSubsystemContext<CandidateBackingMessage>, SubsystemError>,
	StatementDistribution:
		Subsystem<OverseerSubsystemContext<StatementDistributionMessage>, SubsystemError>,
	AvailabilityDistribution:
		Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>, SubsystemError>,
	AvailabilityRecovery:
		Subsystem<OverseerSubsystemContext<AvailabilityRecoveryMessage>, SubsystemError>,
	BitfieldSigning: Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>, SubsystemError>,
	BitfieldDistribution:
		Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>, SubsystemError>,
	Provisioner: Subsystem<OverseerSubsystemContext<ProvisionerMessage>, SubsystemError>,
	RuntimeApi: Subsystem<OverseerSubsystemContext<RuntimeApiMessage>, SubsystemError>,
	AvailabilityStore:
		Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>, SubsystemError>,
	NetworkBridge: Subsystem<OverseerSubsystemContext<NetworkBridgeMessage>, SubsystemError>,
	ChainApi: Subsystem<OverseerSubsystemContext<ChainApiMessage>, SubsystemError>,
	CollationGeneration:
		Subsystem<OverseerSubsystemContext<CollationGenerationMessage>, SubsystemError>,
	CollatorProtocol: Subsystem<OverseerSubsystemContext<CollatorProtocolMessage>, SubsystemError>,
	ApprovalDistribution:
		Subsystem<OverseerSubsystemContext<ApprovalDistributionMessage>, SubsystemError>,
	ApprovalVoting: Subsystem<OverseerSubsystemContext<ApprovalVotingMessage>, SubsystemError>,
	GossipSupport: Subsystem<OverseerSubsystemContext<GossipSupportMessage>, SubsystemError>,
	DisputeCoordinator:
		Subsystem<OverseerSubsystemContext<DisputeCoordinatorMessage>, SubsystemError>,
	DisputeParticipation:
		Subsystem<OverseerSubsystemContext<DisputeParticipationMessage>, SubsystemError>,
	DisputeDistribution:
		Subsystem<OverseerSubsystemContext<DisputeDistributionMessage>, SubsystemError>,
	ChainSelection: Subsystem<OverseerSubsystemContext<ChainSelectionMessage>, SubsystemError>,
{
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_candidate_validation<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		NEW,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		CandidateValidation: 'static,
		F: 'static + FnOnce(CandidateValidation) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<CandidateValidationMessage>,
			SubsystemError,
		>,
	{
		let Self {
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match candidate_validation {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			NEW,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			candidate_validation: replacement,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_candidate_backing<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		NEW,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		CandidateBacking: 'static,
		F: 'static + FnOnce(CandidateBacking) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<CandidateBackingMessage>,
			SubsystemError,
		>,
	{
		let Self {
			candidate_backing,
			candidate_validation,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match candidate_backing {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			NEW,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			candidate_backing: replacement,
			candidate_validation,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_statement_distribution<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		NEW,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		StatementDistribution: 'static,
		F: 'static + FnOnce(StatementDistribution) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<StatementDistributionMessage>,
			SubsystemError,
		>,
	{
		let Self {
			statement_distribution,
			candidate_validation,
			candidate_backing,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match statement_distribution {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			NEW,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			statement_distribution: replacement,
			candidate_validation,
			candidate_backing,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_availability_distribution<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		NEW,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		AvailabilityDistribution: 'static,
		F: 'static + FnOnce(AvailabilityDistribution) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<AvailabilityDistributionMessage>,
			SubsystemError,
		>,
	{
		let Self {
			availability_distribution,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match availability_distribution {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			NEW,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			availability_distribution: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_availability_recovery<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		NEW,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		AvailabilityRecovery: 'static,
		F: 'static + FnOnce(AvailabilityRecovery) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<AvailabilityRecoveryMessage>,
			SubsystemError,
		>,
	{
		let Self {
			availability_recovery,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match availability_recovery {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			NEW,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			availability_recovery: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_bitfield_signing<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		NEW,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		BitfieldSigning: 'static,
		F: 'static + FnOnce(BitfieldSigning) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<BitfieldSigningMessage>,
			SubsystemError,
		>,
	{
		let Self {
			bitfield_signing,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match bitfield_signing {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			NEW,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			bitfield_signing: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_bitfield_distribution<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		NEW,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		BitfieldDistribution: 'static,
		F: 'static + FnOnce(BitfieldDistribution) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<BitfieldDistributionMessage>,
			SubsystemError,
		>,
	{
		let Self {
			bitfield_distribution,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match bitfield_distribution {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			NEW,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			bitfield_distribution: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_provisioner<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		NEW,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		Provisioner: 'static,
		F: 'static + FnOnce(Provisioner) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<ProvisionerMessage>,
			SubsystemError,
		>,
	{
		let Self {
			provisioner,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match provisioner {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			NEW,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			provisioner: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_runtime_api<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		NEW,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		RuntimeApi: 'static,
		F: 'static + FnOnce(RuntimeApi) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<RuntimeApiMessage>,
			SubsystemError,
		>,
	{
		let Self {
			runtime_api,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match runtime_api {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			NEW,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			runtime_api: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_availability_store<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		NEW,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		AvailabilityStore: 'static,
		F: 'static + FnOnce(AvailabilityStore) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<AvailabilityStoreMessage>,
			SubsystemError,
		>,
	{
		let Self {
			availability_store,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match availability_store {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			NEW,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			availability_store: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_network_bridge<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NEW,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		NetworkBridge: 'static,
		F: 'static + FnOnce(NetworkBridge) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<NetworkBridgeMessage>,
			SubsystemError,
		>,
	{
		let Self {
			network_bridge,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match network_bridge {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NEW,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			network_bridge: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_chain_api<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		NEW,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		ChainApi: 'static,
		F: 'static + FnOnce(ChainApi) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<ChainApiMessage>,
			SubsystemError,
		>,
	{
		let Self {
			chain_api,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match chain_api {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			NEW,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			chain_api: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_collation_generation<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		NEW,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		CollationGeneration: 'static,
		F: 'static + FnOnce(CollationGeneration) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<CollationGenerationMessage>,
			SubsystemError,
		>,
	{
		let Self {
			collation_generation,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match collation_generation {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			NEW,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			collation_generation: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_collator_protocol<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		NEW,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		CollatorProtocol: 'static,
		F: 'static + FnOnce(CollatorProtocol) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<CollatorProtocolMessage>,
			SubsystemError,
		>,
	{
		let Self {
			collator_protocol,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match collator_protocol {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			NEW,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			collator_protocol: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_approval_distribution<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		NEW,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		ApprovalDistribution: 'static,
		F: 'static + FnOnce(ApprovalDistribution) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<ApprovalDistributionMessage>,
			SubsystemError,
		>,
	{
		let Self {
			approval_distribution,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match approval_distribution {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			NEW,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			approval_distribution: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_approval_voting<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		NEW,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		ApprovalVoting: 'static,
		F: 'static + FnOnce(ApprovalVoting) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<ApprovalVotingMessage>,
			SubsystemError,
		>,
	{
		let Self {
			approval_voting,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match approval_voting {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			NEW,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			approval_voting: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_gossip_support<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		NEW,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		GossipSupport: 'static,
		F: 'static + FnOnce(GossipSupport) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<GossipSupportMessage>,
			SubsystemError,
		>,
	{
		let Self {
			gossip_support,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match gossip_support {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			NEW,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			gossip_support: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_dispute_coordinator<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		NEW,
		DisputeParticipation,
		DisputeDistribution,
		ChainSelection,
	>
	where
		DisputeCoordinator: 'static,
		F: 'static + FnOnce(DisputeCoordinator) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<DisputeCoordinatorMessage>,
			SubsystemError,
		>,
	{
		let Self {
			dispute_coordinator,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match dispute_coordinator {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			NEW,
			DisputeParticipation,
			DisputeDistribution,
			ChainSelection,
		> {
			dispute_coordinator: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_participation,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_dispute_participation<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		NEW,
		DisputeDistribution,
		ChainSelection,
	>
	where
		DisputeParticipation: 'static,
		F: 'static + FnOnce(DisputeParticipation) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<DisputeParticipationMessage>,
			SubsystemError,
		>,
	{
		let Self {
			dispute_participation,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match dispute_participation {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			NEW,
			DisputeDistribution,
			ChainSelection,
		> {
			dispute_participation: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_distribution,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_dispute_distribution<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		NEW,
		ChainSelection,
	>
	where
		DisputeDistribution: 'static,
		F: 'static + FnOnce(DisputeDistribution) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<DisputeDistributionMessage>,
			SubsystemError,
		>,
	{
		let Self {
			dispute_distribution,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match dispute_distribution {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			NEW,
			ChainSelection,
		> {
			dispute_distribution: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			chain_selection,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
	#[doc = r" Replace a subsystem by another implementation for the"]
	#[doc = r" consumable message type."]
	pub fn replace_chain_selection<NEW, E, F>(
		self,
		gen_replacement_fn: F,
	) -> OverseerBuilder<
		S,
		SupportsParachains,
		CandidateValidation,
		CandidateBacking,
		StatementDistribution,
		AvailabilityDistribution,
		AvailabilityRecovery,
		BitfieldSigning,
		BitfieldDistribution,
		Provisioner,
		RuntimeApi,
		AvailabilityStore,
		NetworkBridge,
		ChainApi,
		CollationGeneration,
		CollatorProtocol,
		ApprovalDistribution,
		ApprovalVoting,
		GossipSupport,
		DisputeCoordinator,
		DisputeParticipation,
		DisputeDistribution,
		NEW,
	>
	where
		ChainSelection: 'static,
		F: 'static + FnOnce(ChainSelection) -> NEW,
		NEW: polkadot_overseer_gen::Subsystem<
			OverseerSubsystemContext<ChainSelectionMessage>,
			SubsystemError,
		>,
	{
		let Self {
			chain_selection,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		} = self;
		let replacement: FieldInitMethod<NEW> = match chain_selection {
			FieldInitMethod::Fn(fx) => {
				FieldInitMethod::Fn(Box::new(move |handle: OverseerHandle| {
					let orig = fx(handle)?;
					Ok(gen_replacement_fn(orig))
				}))
			}
			FieldInitMethod::Value(val) => FieldInitMethod::Value(gen_replacement_fn(val)),
			FieldInitMethod::Uninitialized => {
				panic!("Must have a value before it can be replaced. qed")
			}
		};
		OverseerBuilder::<
			S,
			SupportsParachains,
			CandidateValidation,
			CandidateBacking,
			StatementDistribution,
			AvailabilityDistribution,
			AvailabilityRecovery,
			BitfieldSigning,
			BitfieldDistribution,
			Provisioner,
			RuntimeApi,
			AvailabilityStore,
			NetworkBridge,
			ChainApi,
			CollationGeneration,
			CollatorProtocol,
			ApprovalDistribution,
			ApprovalVoting,
			GossipSupport,
			DisputeCoordinator,
			DisputeParticipation,
			DisputeDistribution,
			NEW,
		> {
			chain_selection: replacement,
			candidate_validation,
			candidate_backing,
			statement_distribution,
			availability_distribution,
			availability_recovery,
			bitfield_signing,
			bitfield_distribution,
			provisioner,
			runtime_api,
			availability_store,
			network_bridge,
			chain_api,
			collation_generation,
			collator_protocol,
			approval_distribution,
			approval_voting,
			gossip_support,
			dispute_coordinator,
			dispute_participation,
			dispute_distribution,
			activation_external_listeners,
			span_per_active_leaf,
			leaves,
			active_leaves,
			supports_parachains,
			known_leaves,
			metrics,
			spawner,
		}
	}
}
#[doc = r" Task kind to launch."]
pub trait TaskKind {
	#[doc = r" Spawn a task, it depends on the implementer if this is blocking or not."]
	fn launch_task<S: SpawnNamed>(
		spawner: &mut S,
		name: &'static str,
		future: BoxFuture<'static, ()>,
	);
}
#[allow(missing_docs)]
struct Regular;
impl TaskKind for Regular {
	fn launch_task<S: SpawnNamed>(
		spawner: &mut S,
		name: &'static str,
		future: BoxFuture<'static, ()>,
	) {
		spawner.spawn(name, future)
	}
}
#[allow(missing_docs)]
struct Blocking;
impl TaskKind for Blocking {
	fn launch_task<S: SpawnNamed>(
		spawner: &mut S,
		name: &'static str,
		future: BoxFuture<'static, ()>,
	) {
		spawner.spawn_blocking(name, future)
	}
}
#[doc = r" Spawn task of kind `self` using spawner `S`."]
pub fn spawn<S, M, TK, Ctx, E, SubSys>(
	spawner: &mut S,
	message_tx: polkadot_overseer_gen::metered::MeteredSender<MessagePacket<M>>,
	signal_tx: polkadot_overseer_gen::metered::MeteredSender<OverseerSignal>,
	unbounded_meter: polkadot_overseer_gen::metered::Meter,
	ctx: Ctx,
	s: SubSys,
	futures: &mut polkadot_overseer_gen::FuturesUnordered<
		BoxFuture<'static, ::std::result::Result<(), SubsystemError>>,
	>,
) -> ::std::result::Result<OverseenSubsystem<M>, SubsystemError>
where
	S: polkadot_overseer_gen::SpawnNamed,
	M: std::fmt::Debug + Send + 'static,
	TK: TaskKind,
	Ctx: polkadot_overseer_gen::SubsystemContext<Message = M>,
	E: std::error::Error + Send + Sync + 'static + From<polkadot_overseer_gen::OverseerError>,
	SubSys: polkadot_overseer_gen::Subsystem<Ctx, E>,
{
	let polkadot_overseer_gen::SpawnedSubsystem::<E> { future, name } = s.start(ctx);
	let (tx, rx) = polkadot_overseer_gen::oneshot::channel();
	let fut = Box::pin(async move {
		if let Err(e) = future.await {
			polkadot_overseer_gen :: tracing :: error!
                (subsystem = name, err = ? e, "subsystem exited with error");
		} else {
			polkadot_overseer_gen::tracing::debug!(
				subsystem = name,
				"subsystem exited without an error"
			);
		}
		let _ = tx.send(());
	});
	<TK as TaskKind>::launch_task(spawner, name, fut);
	futures.push(Box::pin(rx.map(|e| {
		tracing :: warn! (err = ? e, "dropping error");
		Ok(())
	})));
	let instance = Some(SubsystemInstance {
		meters: polkadot_overseer_gen::SubsystemMeters {
			unbounded: unbounded_meter,
			bounded: message_tx.meter().clone(),
			signals: signal_tx.meter().clone(),
		},
		tx_signal: signal_tx,
		tx_bounded: message_tx,
		signals_received: 0,
		name,
	});
	Ok(OverseenSubsystem { instance })
}
#[doc = r" A subsystem that the overseer oversees."]
#[doc = r""]
#[doc = r" Ties together the [`Subsystem`] itself and it's running instance"]
#[doc = r" (which may be missing if the [`Subsystem`] is not running at the moment"]
#[doc = r" for whatever reason)."]
#[doc = r""]
#[doc = r" [`Subsystem`]: trait.Subsystem.html"]
pub struct OverseenSubsystem<M> {
	#[doc = r" The instance."]
	pub instance: std::option::Option<polkadot_overseer_gen::SubsystemInstance<M, OverseerSignal>>,
}
impl<M> OverseenSubsystem<M> {
	#[doc = r" Send a message to the wrapped subsystem."]
	#[doc = r""]
	#[doc = r" If the inner `instance` is `None`, nothing is happening."]
	pub async fn send_message2(
		&mut self,
		message: M,
		origin: &'static str,
	) -> ::std::result::Result<(), SubsystemError> {
		const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);
		if let Some(ref mut instance) = self.instance {
			match instance
				.tx_bounded
				.send(MessagePacket {
					signals_received: instance.signals_received,
					message: message.into(),
				})
				.timeout(MESSAGE_TIMEOUT)
				.await
			{
				None => {
					polkadot_overseer_gen :: tracing :: error!
                    (target : LOG_TARGET, % origin,
                     "Subsystem {} appears unresponsive.", instance.name,);
					Err(SubsystemError::from(
						polkadot_overseer_gen::OverseerError::SubsystemStalled(instance.name),
					))
				}
				Some(res) => res.map_err(Into::into),
			}
		} else {
			Ok(())
		}
	}
	#[doc = r" Send a signal to the wrapped subsystem."]
	#[doc = r""]
	#[doc = r" If the inner `instance` is `None`, nothing is happening."]
	pub async fn send_signal(
		&mut self,
		signal: OverseerSignal,
	) -> ::std::result::Result<(), SubsystemError> {
		const SIGNAL_TIMEOUT: ::std::time::Duration = ::std::time::Duration::from_secs(10);
		if let Some(ref mut instance) = self.instance {
			match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
				None => Err(SubsystemError::from(
					polkadot_overseer_gen::OverseerError::SubsystemStalled(instance.name),
				)),
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
#[doc = r" Collection of channels to the individual subsystems."]
#[doc = r""]
#[doc = r" Naming is from the point of view of the overseer."]
#[derive(Debug, Clone)]
pub struct ChannelsOut {
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub candidate_validation:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<CandidateValidationMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub candidate_backing:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<CandidateBackingMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub statement_distribution:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<StatementDistributionMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub availability_distribution: polkadot_overseer_gen::metered::MeteredSender<
		MessagePacket<AvailabilityDistributionMessage>,
	>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub availability_recovery:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<AvailabilityRecoveryMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub bitfield_signing:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<BitfieldSigningMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub bitfield_distribution:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<BitfieldDistributionMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub provisioner:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<ProvisionerMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub runtime_api:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<RuntimeApiMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub availability_store:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<AvailabilityStoreMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub network_bridge:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<NetworkBridgeMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub chain_api: polkadot_overseer_gen::metered::MeteredSender<MessagePacket<ChainApiMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub collation_generation:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<CollationGenerationMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub collator_protocol:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<CollatorProtocolMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub approval_distribution:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<ApprovalDistributionMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub approval_voting:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<ApprovalVotingMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub gossip_support:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<GossipSupportMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub dispute_coordinator:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<DisputeCoordinatorMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub dispute_participation:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<DisputeParticipationMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub dispute_distribution:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<DisputeDistributionMessage>>,
	#[doc = r" Bounded channel sender, connected to a subsystem."]
	pub chain_selection:
		polkadot_overseer_gen::metered::MeteredSender<MessagePacket<ChainSelectionMessage>>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub candidate_validation_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<CandidateValidationMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub candidate_backing_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<CandidateBackingMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub statement_distribution_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<StatementDistributionMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub availability_distribution_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<AvailabilityDistributionMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub availability_recovery_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<AvailabilityRecoveryMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub bitfield_signing_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<BitfieldSigningMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub bitfield_distribution_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<BitfieldDistributionMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub provisioner_unbounded:
		polkadot_overseer_gen::metered::UnboundedMeteredSender<MessagePacket<ProvisionerMessage>>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub runtime_api_unbounded:
		polkadot_overseer_gen::metered::UnboundedMeteredSender<MessagePacket<RuntimeApiMessage>>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub availability_store_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<AvailabilityStoreMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub network_bridge_unbounded:
		polkadot_overseer_gen::metered::UnboundedMeteredSender<MessagePacket<NetworkBridgeMessage>>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub chain_api_unbounded:
		polkadot_overseer_gen::metered::UnboundedMeteredSender<MessagePacket<ChainApiMessage>>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub collation_generation_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<CollationGenerationMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub collator_protocol_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<CollatorProtocolMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub approval_distribution_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<ApprovalDistributionMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub approval_voting_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<ApprovalVotingMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub gossip_support_unbounded:
		polkadot_overseer_gen::metered::UnboundedMeteredSender<MessagePacket<GossipSupportMessage>>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub dispute_coordinator_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<DisputeCoordinatorMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub dispute_participation_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<DisputeParticipationMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub dispute_distribution_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<DisputeDistributionMessage>,
	>,
	#[doc = r" Unbounded channel sender, connected to a subsystem."]
	pub chain_selection_unbounded: polkadot_overseer_gen::metered::UnboundedMeteredSender<
		MessagePacket<ChainSelectionMessage>,
	>,
}
impl ChannelsOut {
	#[doc = r" Send a message via a bounded channel."]
	pub async fn send_and_log_error(&mut self, signals_received: usize, message: AllMessages) {
		let res: ::std::result::Result<_, _> = match message {
			AllMessages::CandidateValidation(inner) => self
				.candidate_validation
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(candidate_validation)),
			AllMessages::CandidateBacking(inner) => self
				.candidate_backing
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(candidate_backing)),
			AllMessages::StatementDistribution(inner) => self
				.statement_distribution
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(statement_distribution)),
			AllMessages::AvailabilityDistribution(inner) => self
				.availability_distribution
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(availability_distribution)),
			AllMessages::AvailabilityRecovery(inner) => self
				.availability_recovery
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(availability_recovery)),
			AllMessages::BitfieldSigning(inner) => self
				.bitfield_signing
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(bitfield_signing)),
			AllMessages::BitfieldDistribution(inner) => self
				.bitfield_distribution
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(bitfield_distribution)),
			AllMessages::Provisioner(inner) => self
				.provisioner
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(provisioner)),
			AllMessages::RuntimeApi(inner) => self
				.runtime_api
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(runtime_api)),
			AllMessages::AvailabilityStore(inner) => self
				.availability_store
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(availability_store)),
			AllMessages::NetworkBridge(inner) => self
				.network_bridge
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(network_bridge)),
			AllMessages::ChainApi(inner) => self
				.chain_api
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(chain_api)),
			AllMessages::CollationGeneration(inner) => self
				.collation_generation
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(collation_generation)),
			AllMessages::CollatorProtocol(inner) => self
				.collator_protocol
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(collator_protocol)),
			AllMessages::ApprovalDistribution(inner) => self
				.approval_distribution
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(approval_distribution)),
			AllMessages::ApprovalVoting(inner) => self
				.approval_voting
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(approval_voting)),
			AllMessages::GossipSupport(inner) => self
				.gossip_support
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(gossip_support)),
			AllMessages::DisputeCoordinator(inner) => self
				.dispute_coordinator
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(dispute_coordinator)),
			AllMessages::DisputeParticipation(inner) => self
				.dispute_participation
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(dispute_participation)),
			AllMessages::DisputeDistribution(inner) => self
				.dispute_distribution
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(dispute_distribution)),
			AllMessages::ChainSelection(inner) => self
				.chain_selection
				.send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.await
				.map_err(|_| stringify!(chain_selection)),
			AllMessages::Empty => Ok(()),
		};
		if let Err(subsystem_name) = res {
			polkadot_overseer_gen::tracing::debug!(
				target: LOG_TARGET,
				"Failed to send (bounded) a message to {} subsystem",
				subsystem_name
			);
		}
	}
	#[doc = r" Send a message to another subsystem via an unbounded channel."]
	pub fn send_unbounded_and_log_error(&self, signals_received: usize, message: AllMessages) {
		let res: ::std::result::Result<_, _> = match message {
			AllMessages::CandidateValidation(inner) => self
				.candidate_validation_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(candidate_validation)),
			AllMessages::CandidateBacking(inner) => self
				.candidate_backing_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(candidate_backing)),
			AllMessages::StatementDistribution(inner) => self
				.statement_distribution_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(statement_distribution)),
			AllMessages::AvailabilityDistribution(inner) => self
				.availability_distribution_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(availability_distribution)),
			AllMessages::AvailabilityRecovery(inner) => self
				.availability_recovery_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(availability_recovery)),
			AllMessages::BitfieldSigning(inner) => self
				.bitfield_signing_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(bitfield_signing)),
			AllMessages::BitfieldDistribution(inner) => self
				.bitfield_distribution_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(bitfield_distribution)),
			AllMessages::Provisioner(inner) => self
				.provisioner_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(provisioner)),
			AllMessages::RuntimeApi(inner) => self
				.runtime_api_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(runtime_api)),
			AllMessages::AvailabilityStore(inner) => self
				.availability_store_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(availability_store)),
			AllMessages::NetworkBridge(inner) => self
				.network_bridge_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(network_bridge)),
			AllMessages::ChainApi(inner) => self
				.chain_api_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(chain_api)),
			AllMessages::CollationGeneration(inner) => self
				.collation_generation_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(collation_generation)),
			AllMessages::CollatorProtocol(inner) => self
				.collator_protocol_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(collator_protocol)),
			AllMessages::ApprovalDistribution(inner) => self
				.approval_distribution_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(approval_distribution)),
			AllMessages::ApprovalVoting(inner) => self
				.approval_voting_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(approval_voting)),
			AllMessages::GossipSupport(inner) => self
				.gossip_support_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(gossip_support)),
			AllMessages::DisputeCoordinator(inner) => self
				.dispute_coordinator_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(dispute_coordinator)),
			AllMessages::DisputeParticipation(inner) => self
				.dispute_participation_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(dispute_participation)),
			AllMessages::DisputeDistribution(inner) => self
				.dispute_distribution_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(dispute_distribution)),
			AllMessages::ChainSelection(inner) => self
				.chain_selection_unbounded
				.unbounded_send(polkadot_overseer_gen::make_packet(signals_received, inner))
				.map_err(|_| stringify!(chain_selection)),
			AllMessages::Empty => Ok(()),
		};
		if let Err(subsystem_name) = res {
			polkadot_overseer_gen::tracing::debug!(
				target: LOG_TARGET,
				"Failed to send_unbounded a message to {} subsystem",
				subsystem_name
			);
		}
	}
}
#[doc = r" Connector to send messages towards all subsystems,"]
#[doc = r" while tracking the which signals where already received."]
#[derive(Debug, Clone)]
pub struct OverseerSubsystemSender {
	#[doc = r" Collection of channels to all subsystems."]
	channels: ChannelsOut,
	#[doc = r" Systemwide tick for which signals were received by all subsystems."]
	signals_received: SignalsReceived,
}
#[doc = r" implementation for wrapping message type..."]
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<AllMessages> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: AllMessages) {
		self.channels.send_and_log_error(self.signals_received.load(), msg).await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = AllMessages> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: AllMessages) {
		self.channels.send_unbounded_and_log_error(self.signals_received.load(), msg);
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<CandidateValidationMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: CandidateValidationMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = CandidateValidationMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: CandidateValidationMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<CandidateBackingMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: CandidateBackingMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = CandidateBackingMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: CandidateBackingMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<StatementDistributionMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: StatementDistributionMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = StatementDistributionMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: StatementDistributionMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<AvailabilityDistributionMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: AvailabilityDistributionMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = AvailabilityDistributionMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: AvailabilityDistributionMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<AvailabilityRecoveryMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: AvailabilityRecoveryMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = AvailabilityRecoveryMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: AvailabilityRecoveryMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<BitfieldSigningMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: BitfieldSigningMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = BitfieldSigningMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: BitfieldSigningMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<BitfieldDistributionMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: BitfieldDistributionMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = BitfieldDistributionMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: BitfieldDistributionMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<ProvisionerMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: ProvisionerMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = ProvisionerMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: ProvisionerMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<RuntimeApiMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: RuntimeApiMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = RuntimeApiMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: RuntimeApiMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<AvailabilityStoreMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: AvailabilityStoreMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = AvailabilityStoreMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: AvailabilityStoreMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<NetworkBridgeMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: NetworkBridgeMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = NetworkBridgeMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: NetworkBridgeMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<ChainApiMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: ChainApiMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = ChainApiMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: ChainApiMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<CollationGenerationMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: CollationGenerationMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = CollationGenerationMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: CollationGenerationMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<CollatorProtocolMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: CollatorProtocolMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = CollatorProtocolMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: CollatorProtocolMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<ApprovalDistributionMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: ApprovalDistributionMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = ApprovalDistributionMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: ApprovalDistributionMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<ApprovalVotingMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: ApprovalVotingMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = ApprovalVotingMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: ApprovalVotingMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<GossipSupportMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: GossipSupportMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = GossipSupportMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: GossipSupportMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<DisputeCoordinatorMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: DisputeCoordinatorMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = DisputeCoordinatorMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: DisputeCoordinatorMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<DisputeParticipationMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: DisputeParticipationMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = DisputeParticipationMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: DisputeParticipationMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<DisputeDistributionMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: DisputeDistributionMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = DisputeDistributionMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: DisputeDistributionMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[polkadot_overseer_gen::async_trait]
impl SubsystemSender<ChainSelectionMessage> for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: ChainSelectionMessage) {
		self.channels
			.send_and_log_error(self.signals_received.load(), AllMessages::from(msg))
			.await;
	}
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = ChainSelectionMessage> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}
	fn send_unbounded_message(&mut self, msg: ChainSelectionMessage) {
		self.channels
			.send_unbounded_and_log_error(self.signals_received.load(), AllMessages::from(msg));
	}
}
#[doc = r" A context type that is given to the [`Subsystem`] upon spawning."]
#[doc = r" It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s"]
#[doc = r" or to spawn it's [`SubsystemJob`]s."]
#[doc = r""]
#[doc = r" [`Overseer`]: struct.Overseer.html"]
#[doc = r" [`Subsystem`]: trait.Subsystem.html"]
#[doc = r" [`SubsystemJob`]: trait.SubsystemJob.html"]
#[derive(Debug)]
#[allow(missing_docs)]
pub struct OverseerSubsystemContext<M> {
	signals: polkadot_overseer_gen::metered::MeteredReceiver<OverseerSignal>,
	messages: SubsystemIncomingMessages<M>,
	to_subsystems: OverseerSubsystemSender,
	to_overseer:
		polkadot_overseer_gen::metered::UnboundedMeteredSender<polkadot_overseer_gen::ToOverseer>,
	signals_received: SignalsReceived,
	pending_incoming: Option<(usize, M)>,
}
impl<M> OverseerSubsystemContext<M> {
	#[doc = r" Create a new context."]
	fn new(
		signals: polkadot_overseer_gen::metered::MeteredReceiver<OverseerSignal>,
		messages: SubsystemIncomingMessages<M>,
		to_subsystems: ChannelsOut,
		to_overseer: polkadot_overseer_gen::metered::UnboundedMeteredSender<
			polkadot_overseer_gen::ToOverseer,
		>,
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
		}
	}
}
#[polkadot_overseer_gen::async_trait]
impl<M: std::fmt::Debug + Send + 'static> polkadot_overseer_gen::SubsystemContext
	for OverseerSubsystemContext<M>
where
	OverseerSubsystemSender: polkadot_overseer_gen::SubsystemSender<AllMessages>,
	AllMessages: From<M>,
{
	type Message = M;
	type Signal = OverseerSignal;
	type Sender = OverseerSubsystemSender;
	type AllMessages = AllMessages;
	type Error = SubsystemError;
	async fn try_recv(
		&mut self,
	) -> ::std::result::Result<Option<FromOverseer<M, OverseerSignal>>, ()> {
		match polkadot_overseer_gen::poll!(self.recv()) {
			polkadot_overseer_gen::Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
			polkadot_overseer_gen::Poll::Pending => Ok(None),
		}
	}
	async fn recv(
		&mut self,
	) -> ::std::result::Result<FromOverseer<M, OverseerSignal>, SubsystemError> {
		loop {
			if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
				if needs_signals_received <= self.signals_received.load() {
					return Ok(polkadot_overseer_gen::FromOverseer::Communication { msg });
				} else {
					self.pending_incoming = Some((needs_signals_received, msg));
					let signal = self.signals.next().await.ok_or(
						polkadot_overseer_gen::OverseerError::Context(
							"Signal channel is terminated and empty.".to_owned(),
						),
					)?;
					self.signals_received.inc();
					return Ok(polkadot_overseer_gen::FromOverseer::Signal(signal));
				}
			}
			let mut await_message = self.messages.next().fuse();
			let mut await_signal = self.signals.next().fuse();
			let signals_received = self.signals_received.load();
			let pending_incoming = &mut self.pending_incoming;
			let from_overseer = polkadot_overseer_gen::futures::select_biased! {
				signal = await_signal =>
				{
					let signal =
					signal.ok_or(polkadot_overseer_gen :: OverseerError ::
								 Context("Signal channel is terminated and empty.".to_owned()))
					? ; polkadot_overseer_gen :: FromOverseer ::
					Signal(signal)
				} msg = await_message =>
				{
					let packet =
					msg.ok_or(polkadot_overseer_gen :: OverseerError ::
							  Context("Message channel is terminated and empty.".to_owned()))
					? ; if packet.signals_received > signals_received
					{
						* pending_incoming =
						Some((packet.signals_received, packet.message)) ;
						continue ;
					} else
					{
						polkadot_overseer_gen :: FromOverseer :: Communication
						{ msg : packet.message }
					}
				}
			};
			if let polkadot_overseer_gen::FromOverseer::Signal(_) = from_overseer {
				self.signals_received.inc();
			}
			return Ok(from_overseer);
		}
	}
	fn sender(&mut self) -> &mut Self::Sender {
		&mut self.to_subsystems
	}
	fn spawn(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> ::std::result::Result<(), SubsystemError> {
		self.to_overseer
			.unbounded_send(polkadot_overseer_gen::ToOverseer::SpawnJob { name, s })
			.map_err(|_| polkadot_overseer_gen::OverseerError::TaskSpawn(name))?;
		Ok(())
	}
	fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> ::std::result::Result<(), SubsystemError> {
		self.to_overseer
			.unbounded_send(polkadot_overseer_gen::ToOverseer::SpawnBlockingJob { name, s })
			.map_err(|_| polkadot_overseer_gen::OverseerError::TaskSpawn(name))?;
		Ok(())
	}
}
#[doc = r" Generated message type wrapper"]
#[allow(missing_docs)]
#[derive(Debug)]
pub enum AllMessages {
	CandidateValidation(CandidateValidationMessage),
	CandidateBacking(CandidateBackingMessage),
	StatementDistribution(StatementDistributionMessage),
	AvailabilityDistribution(AvailabilityDistributionMessage),
	AvailabilityRecovery(AvailabilityRecoveryMessage),
	BitfieldSigning(BitfieldSigningMessage),
	BitfieldDistribution(BitfieldDistributionMessage),
	Provisioner(ProvisionerMessage),
	RuntimeApi(RuntimeApiMessage),
	AvailabilityStore(AvailabilityStoreMessage),
	NetworkBridge(NetworkBridgeMessage),
	ChainApi(ChainApiMessage),
	CollationGeneration(CollationGenerationMessage),
	CollatorProtocol(CollatorProtocolMessage),
	ApprovalDistribution(ApprovalDistributionMessage),
	ApprovalVoting(ApprovalVotingMessage),
	GossipSupport(GossipSupportMessage),
	DisputeCoordinator(DisputeCoordinatorMessage),
	DisputeParticipation(DisputeParticipationMessage),
	DisputeDistribution(DisputeDistributionMessage),
	ChainSelection(ChainSelectionMessage),
	Empty,
}
impl ::std::convert::From<()> for AllMessages {
	fn from(_: ()) -> Self {
		AllMessages::Empty
	}
}
impl ::std::convert::From<CandidateValidationMessage> for AllMessages {
	fn from(message: CandidateValidationMessage) -> Self {
		AllMessages::CandidateValidation(message)
	}
}
impl ::std::convert::From<CandidateBackingMessage> for AllMessages {
	fn from(message: CandidateBackingMessage) -> Self {
		AllMessages::CandidateBacking(message)
	}
}
impl ::std::convert::From<StatementDistributionMessage> for AllMessages {
	fn from(message: StatementDistributionMessage) -> Self {
		AllMessages::StatementDistribution(message)
	}
}
impl ::std::convert::From<AvailabilityDistributionMessage> for AllMessages {
	fn from(message: AvailabilityDistributionMessage) -> Self {
		AllMessages::AvailabilityDistribution(message)
	}
}
impl ::std::convert::From<AvailabilityRecoveryMessage> for AllMessages {
	fn from(message: AvailabilityRecoveryMessage) -> Self {
		AllMessages::AvailabilityRecovery(message)
	}
}
impl ::std::convert::From<BitfieldSigningMessage> for AllMessages {
	fn from(message: BitfieldSigningMessage) -> Self {
		AllMessages::BitfieldSigning(message)
	}
}
impl ::std::convert::From<BitfieldDistributionMessage> for AllMessages {
	fn from(message: BitfieldDistributionMessage) -> Self {
		AllMessages::BitfieldDistribution(message)
	}
}
impl ::std::convert::From<ProvisionerMessage> for AllMessages {
	fn from(message: ProvisionerMessage) -> Self {
		AllMessages::Provisioner(message)
	}
}
impl ::std::convert::From<RuntimeApiMessage> for AllMessages {
	fn from(message: RuntimeApiMessage) -> Self {
		AllMessages::RuntimeApi(message)
	}
}
impl ::std::convert::From<AvailabilityStoreMessage> for AllMessages {
	fn from(message: AvailabilityStoreMessage) -> Self {
		AllMessages::AvailabilityStore(message)
	}
}
impl ::std::convert::From<NetworkBridgeMessage> for AllMessages {
	fn from(message: NetworkBridgeMessage) -> Self {
		AllMessages::NetworkBridge(message)
	}
}
impl ::std::convert::From<ChainApiMessage> for AllMessages {
	fn from(message: ChainApiMessage) -> Self {
		AllMessages::ChainApi(message)
	}
}
impl ::std::convert::From<CollationGenerationMessage> for AllMessages {
	fn from(message: CollationGenerationMessage) -> Self {
		AllMessages::CollationGeneration(message)
	}
}
impl ::std::convert::From<CollatorProtocolMessage> for AllMessages {
	fn from(message: CollatorProtocolMessage) -> Self {
		AllMessages::CollatorProtocol(message)
	}
}
impl ::std::convert::From<ApprovalDistributionMessage> for AllMessages {
	fn from(message: ApprovalDistributionMessage) -> Self {
		AllMessages::ApprovalDistribution(message)
	}
}
impl ::std::convert::From<ApprovalVotingMessage> for AllMessages {
	fn from(message: ApprovalVotingMessage) -> Self {
		AllMessages::ApprovalVoting(message)
	}
}
impl ::std::convert::From<GossipSupportMessage> for AllMessages {
	fn from(message: GossipSupportMessage) -> Self {
		AllMessages::GossipSupport(message)
	}
}
impl ::std::convert::From<DisputeCoordinatorMessage> for AllMessages {
	fn from(message: DisputeCoordinatorMessage) -> Self {
		AllMessages::DisputeCoordinator(message)
	}
}
impl ::std::convert::From<DisputeParticipationMessage> for AllMessages {
	fn from(message: DisputeParticipationMessage) -> Self {
		AllMessages::DisputeParticipation(message)
	}
}
impl ::std::convert::From<DisputeDistributionMessage> for AllMessages {
	fn from(message: DisputeDistributionMessage) -> Self {
		AllMessages::DisputeDistribution(message)
	}
}
impl ::std::convert::From<ChainSelectionMessage> for AllMessages {
	fn from(message: ChainSelectionMessage) -> Self {
		AllMessages::ChainSelection(message)
	}
}
impl AllMessages {
	#[doc = r" Generated dispatch iterator generator."]
	pub fn dispatch_iter(
		extern_msg: NetworkBridgeEvent<protocol_v1::ValidationProtocol>,
	) -> impl Iterator<Item = Self> + Send {
		::std::array::IntoIter::new([
			extern_msg.focus().ok().map(|event| {
				AllMessages::StatementDistribution(StatementDistributionMessage::from(event))
			}),
			extern_msg.focus().ok().map(|event| {
				AllMessages::BitfieldDistribution(BitfieldDistributionMessage::from(event))
			}),
			extern_msg.focus().ok().map(|event| {
				AllMessages::ApprovalDistribution(ApprovalDistributionMessage::from(event))
			}),
		])
		.into_iter()
		.filter_map(|x: Option<_>| x)
	}
}
