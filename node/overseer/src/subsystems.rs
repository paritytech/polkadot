
use ::polkadot_overseer_gen::{
	MapSubsystem, SubsystemContext,
	Subsystem,
	SubsystemError,
	SpawnedSubsystem,
	FromOverseer,
};

use crate::OverseerSignal;

/// A dummy subsystem that implements [`Subsystem`] for all
/// types of messages. Used for tests or as a placeholder.
#[derive(Clone, Copy, Debug)]
pub struct DummySubsystem;

impl<Context> Subsystem<Context, SubsystemError> for DummySubsystem
where
	Context: SubsystemContext<Signal=OverseerSignal>,
	<Context as SubsystemContext>::Message: std::fmt::Debug + Send + 'static,
{
	fn start(self, mut ctx: Context) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Err(_) => return Ok(()),
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(overseer_msg) => {
						tracing::debug!(
							target: "dummy-subsystem",
							"Discarding a message sent from overseer {:?}",
							overseer_msg
						);
						continue;
					}
				}
			}
		});

		SpawnedSubsystem {
			name: "dummy-subsystem",
			future,
		}
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
#[derive(Debug, Clone)]
pub struct AllSubsystems<
	CV = (), CB = (), CS = (), SD = (), AD = (), AR = (), BS = (), BD = (), P = (),
	RA = (), AS = (), NB = (), CA = (), CG = (), CP = (), ApD = (), ApV = (),
	GS = (),
> {
	/// A candidate validation subsystem.
	pub candidate_validation: CV,
	/// A candidate backing subsystem.
	pub candidate_backing: CB,
	/// A candidate selection subsystem.
	pub candidate_selection: CS,
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

impl<CV, CB, CS, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>
	AllSubsystems<CV, CB, CS, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>
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
		DummySubsystem,
	> {
		AllSubsystems {
			candidate_validation: DummySubsystem,
			candidate_backing: DummySubsystem,
			candidate_selection: DummySubsystem,
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

	pub fn as_ref(&self) -> AllSubsystems<&'_ CV, &'_ CB, &'_ CS, &'_ SD, &'_ AD, &'_ AR, &'_ BS, &'_ BD, &'_ P, &'_ RA, &'_ AS, &'_ NB, &'_ CA, &'_ CG, &'_ CP, &'_ ApD, &'_ ApV, &'_ GS> {
		AllSubsystems {
			candidate_validation: &self.candidate_validation,
			candidate_backing: &self.candidate_backing,
			candidate_selection: &self.candidate_selection,
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

	pub fn map_subsystems<M>(self, m: M)
		-> AllSubsystems<
			<M as MapSubsystem<CV>>::Output,
			<M as MapSubsystem<CB>>::Output,
			<M as MapSubsystem<CS>>::Output,
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
		M: MapSubsystem<CS>,
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
			candidate_selection: m.map_subsystem(self.candidate_selection),
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

pub type AllSubsystemsSame<T> = AllSubsystems<
	T, T, T, T, T,
    T, T, T, T, T,
    T, T, T, T, T,
    T, T, T,
>;
