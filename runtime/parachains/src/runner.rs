use super::*;
use crate::{builder::{Bench, BenchBuilder}, mock::{new_test_ext, MockGenesisConfig, Test}, paras, paras_inherent, session_info};
use frame_support::assert_ok;
use crate::configuration::HostConfiguration;
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_support::pallet_prelude::*;
use primitives::v1::{
    collator_signature_payload, AvailabilityBitfield, BackedCandidate, CandidateCommitments,
    CandidateDescriptor, CandidateHash, CollatorId, CollatorSignature, CommittedCandidateReceipt,
    CompactStatement, CoreIndex, CoreOccupied, DisputeStatement, DisputeStatementSet, GroupIndex,
    HeadData, Id as ParaId, InherentData as ParachainsInherentData, InvalidDisputeStatementKind,
    PersistedValidationData, SessionIndex, SigningContext, UncheckedSigned,
    ValidDisputeStatementKind, ValidationCode, ValidatorId, ValidatorIndex, ValidityAttestation,
};
use sp_core::{sr25519, H256};
use sp_runtime::{
    generic::Digest,
    traits::{Header as HeaderT, One, TrailingZeroInput, Zero},
    RuntimeAppPublic,
};
use sp_std::{collections::btree_map::BTreeMap, convert::TryInto, prelude::Vec, vec};

pub struct PalletRunner<T> {
    pallet_config: PhantomData<T>
}

// Note: initalizer config aggregates a lot of different pallets configs, as well
// as frame_system::Config
impl<C: initializer::pallet::Config + paras_inherent::pallet::Config> PalletRunner<C> {
    pub fn init() {
        // Make sure relevant storage is cleared. This is just to get the asserts to work when
        // running tests because it seems the storage is not cleared in between.
        inclusion::PendingAvailabilityCommitments::<C>::remove_all(None);
        inclusion::PendingAvailability::<C>::remove_all(None);
    }

    /// Runs all hooks related to block execution until the target
    /// block is reached
    /// Skipping inherent hooks is needed for the bench builder to work.
    ///  TODO: need to explore how not skipping those hooks would affect benchmarks
    pub fn run_to_block(to: C::BlockNumber, skip_inherent_hooks: bool) {
        while Self::current_block_number() < to {
            Self::run_to_next_block(skip_inherent_hooks);
        }
    }

    /// Same as BenchBuilder::setup_session, but based off the current block
    /// to make it possible to trigger multiple sessions during the test
    /// Skipping inherent hooks is needed for the bench builder to work.
    /// TODO: need to explore how not skipping those hooks would affect benchmarks
    pub fn run_to_session(
        target_session: SessionIndex,
        validators: Vec<(C::AccountId, ValidatorId)>,
        total_cores: u32,
        skip_inherent_hooks: bool,
    ) -> (Vec<ValidatorId>, C::BlockNumber, SessionIndex) {
        let current_session_index = Self::current_session_index();
        for session_index in current_session_index..=target_session {
            // Triggers new session in the pallet - this is not implemented in the mock,
            // so needs to be called manually here
            initializer::Pallet::<C>::test_trigger_on_new_session(
                false,
                session_index,
                validators.iter().map(|(a, v)| (a, v.clone())),
                None,
            );
            Self::run_to_next_block(skip_inherent_hooks);
        }

        let block_number = Self::current_block_number();
        let header = BenchBuilder::<C>::header(block_number.clone());

        // Starts the execution of a particular block
        frame_system::Pallet::<C>::initialize(
            &header.number(),
            &header.hash(),
            &Digest { logs: Vec::new() },
            Default::default(),
        );

        assert_eq!(<shared::Pallet<C>>::session_index(), target_session);

        // We need to refetch validators since they have been shuffled.
        let validators_shuffled: Vec<_> = session_info::Pallet::<C>::session_info(target_session)
            .unwrap()
            .validators
            .clone();

        // self.validators = Some(validators_shuffled);
        // self.block_number = block_number;
        // self.session = target_session;
        assert_eq!(paras::Pallet::<C>::parachains().len(), total_cores as usize);
        (validators_shuffled, block_number, target_session)
    }

    pub fn active_validators_for_session(session_index: SessionIndex) -> Vec<ValidatorId> {
    	session_info::Pallet::<C>::session_info(session_index)
    		.unwrap()
    		.validators
    		.clone()
    }

    /// Calls finalization hooks for the current block and
    /// initialization hooks for the next block.
    /// Skipping inherent hooks is needed for the bench builder to work.
    ///  TODO: need to explore how not skipping those hooks would affect benchmarks
    pub fn run_to_next_block(skip_inherent_hooks: bool) {
        // Finalizing current block
        // NOTE: this has to be specifically the initializer, no frame_system,
        // otherwise initializer doesn't get triggered
        initializer::Pallet::<C>::on_finalize(Self::current_block_number());

        // skipping inherent hooks is needed for the bench builder to work.
        // need to explore how not skipping those hooks would affect benchmarks
        if !skip_inherent_hooks {
            // Finalizing inherent
            paras_inherent::Pallet::<C>::on_finalize(Self::current_block_number());
        }

        let next_block_number = Self::current_block_number() + One::one();
        frame_system::Pallet::<C>::set_block_number(next_block_number.clone());
        // Initializing next block
        initializer::Pallet::<C>::on_initialize(next_block_number.clone());

        if !skip_inherent_hooks {
            // Initializing inherent
            paras_inherent::Pallet::<C>::on_initialize(next_block_number.clone());
        }
    }

    pub fn inherent_data_for_current_block() -> ParachainsInherentData {

    }

    pub fn add_backed_candidate() -> {

    }

    pub fn dispute_block() -> {

    }

    pub fn create_availability_bitfields_for_session(session_index: SessionIndex) -> Vec<UncheckedSigned<AvailabilityBitfield>> {
        let validators = Self::active_validators_for_session(session_index);

        let availability_bitvec = Self::availability_bitvec(concluding_cores, total_cores);

        let bitfields: Vec<UncheckedSigned<AvailabilityBitfield>> = validators
            .iter()
            .enumerate()
            .map(|(i, public)| {
                let unchecked_signed = UncheckedSigned::<AvailabilityBitfield>::benchmark_sign(
                    public,
                    availability_bitvec.clone(),
                    &self.signing_context(),
                    ValidatorIndex(i as u32),
                );

                unchecked_signed
            })
            .collect();

        for (seed, _) in concluding_cores.iter() {
            // make sure the candidates that will be concluding are marked as pending availability.
            let (para_id, core_idx, group_idx) = Self::create_indexes(seed.clone());
            BenchBuilder::<C>::add_availability(
                para_id,
                core_idx,
                group_idx,
                Self::validator_availability_votes_yes(validators.len()),
                CandidateHash(H256::from(byte32_slice_from(seed.clone()))),
            );
        }

        bitfields
    }

    /// Create para id, core index, and grab the associated group index from the scheduler pallet.
    fn create_indexes(seed: u32) -> (ParaId, CoreIndex, GroupIndex) {
        let para_id = ParaId::from(seed);
        let core_idx = CoreIndex(seed);
        let group_idx =
            scheduler::Pallet::<C>::group_assigned_to_core(core_idx, Self::current_block_number()).unwrap();

        (para_id, core_idx, group_idx)
    }

    /// Returns the current block number
    pub fn current_block_number() -> C::BlockNumber {
        frame_system::Pallet::<C>::block_number()
    }

    pub fn current_session_index() -> SessionIndex {
        <shared::Pallet<C>>::session_index()
    }

    // fn run_to_session(n: u32) {
    // 	let block_number = BLOCKS_PER_SESSION * n;
    // 	Self::run_to_block(block_number);
    // }

    /// Returns the last deposited event
    pub fn last_event() -> <C as frame_system::Config>::Event {
        frame_system::Pallet::<C>::events().pop().expect("Event expected").event
    }

    /// Checks whether the event was deposited or not
    pub fn contains_event(generic_event: frame_system::Event<C>) -> bool {
        let event: <C as frame_system::Config>::Event = generic_event.into();
        frame_system::Pallet::<C>::events().iter().any(|x| x.event == event)
    }

    pub fn assert_last_event(generic_event: frame_system::Event<C>) {
        let events = frame_system::Pallet::<C>::events();
        let system_event: <C as frame_system::Config>::Event = generic_event.into();
        // compare to the last event record
        let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
        assert_eq!(event, &system_event);
    }
}