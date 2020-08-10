//! Chain stories supporting approval assignment criteria
//!
//! We compute approval checker assignment criteria with VRF outputs,
//! but their correxponding VRF inputs come from information that
//! ideally lives on-chain.  In this submodule, we either retrieve
//! such information provided it exists on-chain, or else revalidate
//! it when it lives off-chain, and then create the "(chain) stories"
//! actually use in validating assignment criteria. 
//! In short, stories isolate our data dependencies upon the relay chain.

use std::sync::Arc;
use std::collections::HashMap;

use babe_primitives::{EquivocationProof, AuthorityId, make_transcript};
use sc_consensus_babe::{Epoch};
// use sc_consensus_slots::{EquivocationProof};
// use sp_consensus_babe::{EquivocationProof};
// https://github.com/paritytech/substrate/pull/6362/files#diff-aee164b6a1b80d52767f208971d01d82R32

use super::{AssignmentResult, DelayTranche, ParaId, Hash, Header, Error, ValidatorId};


/// A slot number.
pub type SlotNumber = u64;

/// A epoch number.
pub type EpochNumber = u64;


pub const ANV_SLOTS_PER_BP_SLOTS: u64 = 12; // = 6*2, so every half second

pub const NOSHOW_DELAY_TRANCHES: super::DelayTranche = 24;  // Twice ANV_SLOTS_PER_BP_SLOTS ??


/// Identifies the relay chain block in which we declared these
/// parachain candidates to be availability 
#[derive(Clone,PartialEq,Eq)]
pub struct ApprovalContext {
    /// Relay chain slot number of availability declaration in the relay chain
    pub(crate) slot: SlotNumber,
    /// Epoch in which slot occurs
    pub(crate) epoch: EpochNumber,
    /// Block hash 
    pub(crate) hash: Hash,
    /// Block producer
    pub(crate) authority: ValidatorId,
}

impl ApprovalContext {
    pub fn anv_slot_number(&self) -> SlotNumber {
        self.slot.checked_mul(ANV_SLOTS_PER_BP_SLOTS)
        .expect("Almost 2^60 seconds elapsed?!?")
    }

    pub fn new(checker: ValidatorId) -> AssignmentResult<ApprovalContext> {
        let slot: u64 = unimplemented!();
        let epoch: u64 = unimplemented!();
        let hash: Hash = unimplemented!();
        let authority: ValidatorId = unimplemented!();
        Ok(ApprovalContext { slot, epoch, hash, authority })
    }

    /// Relay chain block production slot
    pub fn slot(&self) -> u64 { self.slot }

    /// Relay chain block production epoch
    pub fn epoch(&self) -> u64 { self.epoch }

    /// Relay chain block hash
    pub fn hash(&self) -> &Hash { &self.hash }

    /// Fetch full epoch data from self.epoch
    pub fn fetch_epoch(&self) -> Epoch {
        unimplemented!()
    }

    /// Fetch full epoch data from self.epoch
    pub fn fetch_header(&self) -> Header {
        unimplemented!()
    }

    /// Assignments of `ParaId` to ailability cores for the current
    /// `epoch` and `slot`.
    /// 
    /// We suggest any full parachains have their cores allocated by
    /// the epoch randomness from BABE, so parachain cores should be
    /// allocated using a permutation, maybe Fisher-Yates shuffle,
    /// seeded by the hash of the babe_epoch_randomness and the
    /// slot divided by some small constant.
    ///
    /// We do not minimize aversarial manipulation for parathreads
    /// similarly however because we operate under the assumption
    /// that an adversary with enough checkers for an attack should
    /// possess block production capability for most parachains.
    /// We still favor scheduling parathreads onto availability cores
    /// earlier rather than later however.
    pub(super) fn paraids_by_core(&self) -> &Arc<[Option<ParaId>]> {
        unimplemented!()
    }

    pub(super) fn paraids(&self) -> impl Iterator<Item=ParaId> + '_ {
        self.paraids_by_core().iter().filter_map(Option::as_ref).cloned()
    }

    /// Assignments of `ParaId` to ailability cores for the current
    /// `epoch` and `slot`.
    ///
    /// TODO: Return `CoreId`.  Improve performance!!!
    pub(super) fn core_by_paraid(&self, paraid: ParaId) -> Option<()> {
        if ! self.paraids_by_core().contains(&Some(paraid)) { return None; }
        Some(())
    }

    /// Availability core supply
    ///
    /// An upper bound on `self.paraids_by_core().len()` that remains
    /// constant within an epoch.  Any changes should obey code change
    /// rules and thus be delayed one epoch.
    pub fn max_cores(&self) -> u32 { 128 }

    /// We set two delay tranches per core so that each tranche expects
    /// half as many checkers as the number of backing checkers.
    pub fn delay_tranches_per_core(&self) -> DelayTranche { 2 }

    /// Maximum number of delay tranches
    ///
    /// We do not set this differently for RelayEquivocationStory and
    /// RelayVRF because doing so complicates the code needlessly, and
    /// this bound should never be reached by either.
    pub fn num_delay_tranches(&self) -> DelayTranche {
        self.max_cores().saturating_mul( self.delay_tranches_per_core() )
    }

    /// We sample in `RingVRFModulo` this many VRF inputs from the 
    /// relay chain VRF to populate our zeroth delay tranche.
    pub fn num_samples(&self) -> u16 { 3 }

    /// Create story for assignment criteria determined by relay chain VRFs.
    ///
    /// We must only revalidate the relay chain VRF, by supplying the proof,
    /// if we have not already done so when importing the block.
    pub fn new_vrf_story(&self)
     -> AssignmentResult<RelayVRFStory> 
    {
        let header = self.fetch_header();
        let vrf_t = make_transcript(
            &self.fetch_epoch().randomness, 
            self.slot, 
            self.epoch // == self.epoch().epoch_index,
        );
        // TODO: Should we check this block's proof again?  I suppose no, but..
        let vrf_proof: Option<&::schnorrkel::vrf::VRFProof> = None;
        
        let authority_id: AuthorityId = unimplemented!();
        use primitives::crypto::Public;
        let publickey = ::schnorrkel::PublicKey::from_bytes(&authority_id.to_raw_vec()) // Vec WTF?!?
            .map_err(|_| Error::BadStory("Relay chain block authorized by improper sr25519 public key")) ?;

        let vrf_preout = unimplemented!();
        let vrf_io = if let Some(pr) = vrf_proof {
            publickey.vrf_verify(vrf_t,vrf_preout,pr)
            .map_err(|_| Error::BadStory("Relay chain block VRF failed validation")) ?.0
        } else {
            unimplemented!(); // TODO: Verify that we imported this block?
            vrf_preout.attach_input_hash(&publickey,vrf_t)
            .map_err(|_| Error::BadStory("Relay chain block with invalid VRF PreOutput")) ?
        };

        let anv_rc_vrf_source = vrf_io.make_bytes::<[u8; 32]>(b"A&V RC-VRF");
        // above should be based on https://github.com/paritytech/substrate/pull/5788/files

        Ok(RelayVRFStory { anv_rc_vrf_source, })
    }

    /// Initalize empty story for a relay chain block with no equivocations so far.
    ///
    /// We must only revalidate the relay chain VRF, by supplying the proof,
    /// if we have not already done so when importing the block.
    pub fn new_equivocation_story(&self) -> RelayEquivocationStory {
        let header = self.fetch_header();
        RelayEquivocationStory { header, relay_equivocations: Vec::new(), candidate_equivocations: HashMap::new() }
    }
}


/// Approval assignment story comprising a relay chain VRF output
pub struct RelayVRFStory {
    /// Actual final VRF output computed from the relay chain VRF
    pub(super) anv_rc_vrf_source: [u8; 32],
}

/// Approval assignments whose availability declaration is an equivocation
pub struct RelayEquivocationStory {
    /// Relay chain block hash
    pub(super) header: Header,
    /// Relay chain equivocations from which we compute candidate equicocations
    pub(super) relay_equivocations: Vec<Header>, 
    /// Actual candidate equicocations and rtheir block hashes
    pub(super) candidate_equivocations: HashMap<ParaId,Hash>,
}

impl RelayEquivocationStory {
    /// Add any candidate equivocations found within a relay chain equivocation.
    ///
    /// We define a candidate equivocation in a relay chain block X as
    /// a candidate declared available in X but not declared available
    /// in some relay chain block production equivocation Y of X.  
    ///
    /// We know all `EquivocationProof`s were created by calls to
    /// `sp_consensus_slots::check_equivocation`, so they represent
    /// real relay chainlock production  bequivocations, and need
    /// not be rechecked here.
    pub fn add_equivocation(&mut self, ep: &EquivocationProof<Header>) 
     -> AssignmentResult<()>
    {
        let slot = ep.slot_number;
        let headers = [&ep.first_header, &ep.second_header];
        let (i,our_header) = headers.iter()
            .enumerate()
            .find( |(_,h)| h.hash() == self.header.hash() )
            .ok_or(Error::BadStory("Cannot add unrelated equivocation proof.")) ?;
        let other_header = headers[1-i];
        self.relay_equivocations.push(other_header.clone());

        unimplemented!() 
        // TODO: Iterate over candidates in our_header and add to self.candidate_equivocations any that differ or do not exist in other_header.  We should restrict to tracker.candidates somehow if initalize_candidate could be called on fewer than all candidates, but we'll leave this code here until we descide if we want that functionality.  
        }
}

