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

//! The dispute module is responsible for resolving disputes that appear after block inclusion.
//!
//! It is responsible for collecting votes from validators after an initial local dispute as well
//! as crafting transactions using the provisioner for slashing the validators on the wrong side.

use sp_std::prelude::*;
use primitives::v1::{
	ValidatorId, CandidateCommitments, CandidateDescriptor, CandidateReceipt, ValidatorIndex, Id as ParaId,
	AvailabilityBitfield as AvailabilityBitfield, SignedAvailabilityBitfields, SigningContext,
	BackedCandidate, CoreIndex, GroupIndex, CommittedCandidateReceipt,
	CandidateReceipt, HeadData,
};
use frame_support::{
	decl_storage, decl_module, decl_error, decl_event, ensure, debug,
	dispatch::DispatchResult, IterableStorageMap, weights::Weight, traits::Get,
};
use codec::{Encode, Decode};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use pallet_staking::SessionIndex;
use pallet_session::Trait as Session;
use pallet_offences::Trait as Offences;
use sp_runtime::{DispatchError, traits::{One, Saturating}};

use crate::{configuration, paras, scheduler::CoreAssignment};

struct DisputeOffence {
    session: SessionIndex,
    offenders: Vec<Offender>,
    validators: Vec<ValidatorId>,
    when: u128, // FIXME chose a sane time base
}

impl<Offender> Offence<Offender> for DisputeOffence {
    const ID:Kind = b"dispute:notgood";

    type TimeSlot: u128;

    fn offenders(&self) -> Vec<Offender> {
        self.offenders.clone()
    }

    fn session_index(&self) -> SessionIndex {
        self.session
    }

    fn validator_set_count(&self) -> u32 {
        self.valdiators.len() as u32
    }

    fn time_slot(&self) -> Self::TimeSlot {
        self.when
    }

    fn slash_fraction(
        offenders_count: u32,
        validator_set_count: u32,
    ) -> Perbill {
        Perbill(1)
    }
}

impl DisputeOffence {

}
pub trait Trait:
	frame_system::Trait + paras::Trait + configuration::Trait
{
    type Event: From<Event<Self>> + Into<<Self as frame_system::Trait>::Event>;
}

decl_storage! {
	trait Store for Module<T: Trait> as Dispute {
		/// The set of validators in favor of a particular block.
		VotePro: map hasher(twox_64_concat) Hash
			=> Vec<ValidatorId>;

        /// The set of validators against a particular block.
        VoteCon: map hasher(twox_64_concat) Hash
			=> Vec<ValidatorId>;

		/// The commitments of candidates pending availability, by `ParaId`.
		Commitments: map hasher(twox_64_concat) ParaId
			=> Option<CandidateCommitments>;
	}
}

// only for 3rd party apps, not for internal usage 
decl_event! {
	pub enum Event<T> where <T as frame_system::Trait>::Hash, <T as frame_system::Trait>::BlockNumber {
        /// An indication of one validator that something is off. []
        DisputeIndicated(CandidateReceipt<Hash>, SessionIndex, BlockNumber), // TODO what over checks must there be included? secondary
		/// A dispute resolved with an outcome. []
		DisputeResolved(CandidateReceipt<Hash>, SessionIndex, BlockNumber),
		/// A candidate timed out. []
		DisputeTimedOut(CandidateReceipt<Hash>, HeadData),
	}
}

decl_module! {
	/// The parachain-candidate dispute module.
	pub struct Module<T: Trait>
		for enum Call where origin: <T as frame_system::Trait>::Origin
	{
        fn deposit_event() = default;


        /// Execute once a candidate becomes available.
        ///
        /// From this point on the period for secondary approval checks
        /// for the para block starts.
        /// Within this period disputes may occur.
        #[weight = (1_000_000_000, DispatchClass::Mandatory)]
        pub fn on_candidate_becomes_available(
            origin,
            para_block_hash: <Self as Trait>::Hash,

        ) -> DispatchResult {
            ensure_none!(origin)?;
            ensure!(Self::local_chain_contains_para_block())?;

            let validators = <T as Session>::Validator::validators()?;

            Ok(())
        }

        /// One secondary validator deems the para-block in question
        /// invalid, and starts a dispute.
        #[weight = (1_000_000_000, DispatchClass::Mandatory)]
        pub fn on_report(origin,
            reporter: ValidatorId,
            report: DisputeReport) -> DispatchResult {

            // the relevant session
            let session = report.session;

            // TODO prevent double votes
            // TODO punish attempted double votes?

            let first = ValidatorCount::exists(); // TODO track if this is the first report for this para-block
            if first {
                // all validators that did already cast their vote,
                // are already sorted into their voting bucket.
                let validators = Self::original_validating_valdiators(session)?;
                VotePro::mutate(move |mut set: Vec<ValidatorId>| { set.extend(validators); Ok(set) } )?;
                VoteCon::mutate(move |mut set: Vec<ValidatorId>| { set.push(reporter); Ok(set) } )?;
            
                let additional = Self::escalate()?;
                // TODO unionize / exclude duplicates
                
                let unioned_validators = unimplemented!();
                
                ValidatorCount::set(union_validators.len());
            } else {
                let all_validators = Self::validator_set().len();
                let DisputeVotes { pro, cons } = Self::count_pro_and_cons_votes(block_hash);
                let thresh = resolution_threshold(all_validators.len()) as u32;
                let (pro, cons) = (pro >= thresh, cons >= thresh);
        
                if pro && cons {
                    unreachable!("The number of validators was correctly assessed. qed");
                } else if !pro && !cons {
                    // nothing todo just yet
                    return Ok(())
                }

                assert!(pro ^ con, "Pro and con can not have super majority at the same time. qed");

                let offenders = if cons {
                    // only revert the block here, the decision is already made
                    // TODO how do we revert a block (?)
                    // TODO transplant to all active heads
                } else {
                    // do not revert the block, everything is fine, just slash the
                    // reporters
                };

            }

            Ok(())
        }

        /// Whenever a secondary approval is passed, this keeps track of it.
        #[weight = (1_000_000_000, DispatchClass::Mandatory)]
        pub fn on_secondary_approval_vote_cast(
            origin,
            disputed_para_block: Hash,
        ) -> DispatchResult {
            ensure_none!(origin)?;

            VotePro::mutate(disputed_para_block, |v| { v.push(validator); Ok(v) } );

            Ok(())
        }


        /// Triggers the slashing at the end of $period or session change.
        // TODO clarify when this is happening exactly.
        #[weight = (1_000_000_000, DispatchClass::Mandatory)]
        pub fn slash_defeated(session: SessionIndex) -> DispatchResult {
            // TODO use the information in the storage in order to
            // TODO find the correct subset of valdiators to slash

            let offence = Offence::new(); // FIXME 
            // TODO reporters are empty
            // TODO eval if we should populate it with all validators on the winning side?
            let reporters = Vec::new();
            Offences::report_offence(offence, offenders)?;
        }

	}
}


impl<T: Trait> Module<T> {

	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight { 0 }

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() { }

	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>
	) {
		// unlike most drain methods, drained elements are not cleared on `Drop` of the iterator
		// and require consumption.
        for _ in <VotePro>::drain() { }
        for _ in <VoteCon>::drain() { }
    }

    /// Query all validators, since there was an initial dispute
    ///
    /// Returns a set of additional validators, which are asked to verify.
    fn escalate() -> Vec<ValidatorId> {
        // TODO collect **all** validators available
        vec![]
    }

    /// Obtain the stored validators that support the block validity claim.
    fn validators_pro() -> Vec<ValidatorId> {
        VotePro::<T>::get()
    }

    /// Obtain the stored validators that are challenging the block validity.
    fn validators_con() -> Vec<ValidatorId> {
        VoteCon::<T>::get()
    }

    /// The set of validators which were responsible in the session
    /// the block was backed in.
    fn original_validating_valdiators(session: SessionIndex) -> Vec<ValidatorId> {
        Session::validators(session)
    }

    /// Check all of the known votes in storage for that block.
    fn count_pro_and_cons_votes(block: <T as frame_system::Trait>::Hash) -> DisputeVotes {
        DisputeVotes {
            pro: validators_pro().len(),
            con: validators_con().len(),
        }
    }

    /// Transplant a vote onto all other forks.
    fn transplant_to(resolution: Resolution, active_heads: Vec<<T as frame_system::Trait>::Hash>) {
        unimplemented!("transplantation is not yet impl'd")
    }

    /// Extend the set of blocks to never sync again.
    fn extend_blacklist(burnt: &[<T as frame_system::Trait>::Hash]) {
        unimplemented!("Use that other module impl")
    }


    /// Do the work for this particular block.
    fn on_initialize(n: <T as frame_system::Trait>::BlockNumber) -> frame_support::weights::Weight {
        
        let events: Vec<EventRecord<<T as frame_system::Trait>::Event, <T as frame_system::Trait>::Hash>> = Events::<T>::events();

        // TODO what to do with the events? this is duplicate information.

        weight
	}

    /// Local disputes can only be processes iff the para-block
    /// is part of the current relay-chain.
    pub fn local_chain_contains_para_block() -> bool {
        unimplemented!("How to achieve the equiv of `HeaderBackend<Block>::number(&client)` from the runtime?")
    }



    /// block the block number in question
    ///
    pub(crate) fn process_concluded_dispute(
        block_number: <T as frame_system::Trait>::BlockNumber,
        block_hash: <T as frame_system::Trait>::Hash,
        session: SessionIndex) -> Result<(), DispatchError>
    {
        // TODO ensure!(..), bounds unclear

        // number of _all_ validators

        let resolution = if cons {
            Self::extend_blacklist(&[block_hash]);
            // slash the other party
            Resolution {
                hash: block_hash,
                to_punish: Self::validators_pro(),
                was_truely_wrong: true,
            }
        } else if pro {
            // slash the other party
            Resolution {
                hash: block_hash,
                to_punish: Self::validators_cons(),
                was_truely_wrong: false,
            }
        } else {
            return;
        };

        // TODO extract from the runtime, is this correct? 
        let active_heads = vec![];

        // 
        Self::transplant_to(resolution, active_heads);

        // TODO slash

        Ok(())
    }


    /// Process an unconcluded 
    fn process_unconcluded_dispute() {
        unimplemented!("XXX");
    }

    /// Check if we are still within the acceptance period, which is equiv to the
    /// code from that block being still around
    // TODO double check
    fn within_acceptance_period<T: Trait>(para_id: ParaId, block: <T as Trait>::BlockNumber) -> bool {
        Paras::validation_code_at(para_id, block, None).is_some()
    }
}

#[derive(Encode, Decode)]
struct Resolution {
    hash: Hash, // hash of para-block this dispute was about
    revert_to: Hash, // hash of the relay chain block to revert back to
    was_truely_wrong: bool, // if the originally tagged as bad, was actually bad
    to_punish: Vec<ValidatorId>, // the validator party to slash
}

#[derive(Encode, Decode, Default)]
pub(crate) struct DisputeVotes {
    pub(crate) pro: u32,
    pub(crate) cons: u32,
}

/// Calculate the majority requred to sway in one way or another
const fn resolution_threshold(n_validators: usize) -> usize {
	n_validators - (n_validators * 1) / 3
}

#[cfg(test)]
mod tests {
    use super::*;
    

    #[test]
    fn f() {
       assert!(true);
    }
}
