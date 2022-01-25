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

//! The paras pallet acts as the main registry of paras.
//!
//! # Tracking State of Paras
//!
//! The most important responsibility of this module is to track which parachains and parathreads
//! are active and what their current state is. The current state of a para consists of the current
//! head data and the current validation code (AKA Parachain Validation Function (PVF)).
//!
//! A para is not considered live until it is registered and activated in this pallet.
//!
//! The set of parachains and parathreads cannot change except at session boundaries. This is
//! primarily to ensure that the number and meaning of bits required for the availability bitfields
//! does not change except at session boundaries.
//!
//! # Validation Code Upgrades
//!
//! When a para signals the validation code upgrade it will be processed by this module. This can
//! be in turn split into more fine grained items:
//!
//! - Part of the acceptance criteria checks if the para can indeed signal an upgrade,
//!
//! - When the candidate is enacted, this module schedules code upgrade, storing the prospective
//!   validation code.
//!
//! - Actually assign the prospective validation code to be the current one after all conditions are
//!   fulfilled.
//!
//! The conditions that must be met before the para can use the new validation code are:
//!
//! 1. The validation code should have been "soaked" in the storage for a given number of blocks. That
//!    is, the validation code should have been stored in on-chain storage for some time, so that in
//!    case of a revert with a non-extreme height difference, that validation code can still be
//!    found on-chain.
//!
//! 2. The validation code was vetted by the validators and declared as non-malicious in a processes
//!    known as PVF pre-checking.
//!
//! # Validation Code Management
//!
//! Potentially, one validation code can be used by several different paras. For example, during
//! initial stages of deployment several paras can use the same "shell" validation code, or
//! there can be shards of the same para that use the same validation code.
//!
//! In case a validation code ceases to have any users it must be pruned from the on-chain storage.
//!
//! # Para Lifecycle Management
//!
//! A para can be in one of the two stable states: it is either a parachain or a parathread.
//!
//! However, in order to get into one of those two states, it must first be onboarded. Onboarding
//! can be only enacted at session boundaries. Onboarding must take at least one full session.
//! Moreover, a brand new validation code should go through the PVF pre-checking process.
//!
//! Once the para is in one of the two stable states, it can switch to the other stable state or to
//! initiate offboarding process. The result of offboarding is removal of all data related to that
//! para.
//!
//! # PVF Pre-checking
//!
//! As was mentioned above, a brand new validation code should go through a process of approval.
//! As part of this process, validators from the active set will take the validation code and
//! check if it is malicious. Once they did that and have their judgement, either accept or reject,
//! they issue a statement in a form of an unsigned extrinsic. This extrinsic is processed by this
//! pallet. Once supermajority is gained for accept, then the process that initiated the check
//! is resumed (as mentioned before this can be either upgrading of validation code or onboarding).
//! If supermajority is gained for reject, then the process is canceled.
//!
//! Below is a state diagram that depicts states of a single PVF pre-checking vote.
//!
//! ```text
//!                                            ┌──────────┐
//!                        supermajority       │          │
//!                    ┌────────for───────────▶│ accepted │
//!        vote────┐   │                       │          │
//!         │      │   │                       └──────────┘
//!         │      │   │
//!         │  ┌───────┐
//!         │  │       │
//!         └─▶│ init  │────supermajority      ┌──────────┐
//!            │       │       against         │          │
//!            └───────┘           └──────────▶│ rejected │
//!             ▲  │                           │          │
//!             │  │ session                   └──────────┘
//!             │  └──change
//!             │     │
//!             │     ▼
//!             ┌─────┐
//! start──────▶│reset│
//!             └─────┘
//! ```
//!

use crate::{configuration, initializer::SessionChangeNotification, shared};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_support::{pallet_prelude::*, traits::EstimateNextSessionRotation};
use frame_system::pallet_prelude::*;
use parity_scale_codec::{Decode, Encode};
use primitives::{
	v1::{
		ConsensusLog, HeadData, Id as ParaId, SessionIndex, UpgradeGoAhead, UpgradeRestriction,
		ValidationCode, ValidationCodeHash, ValidatorSignature,
	},
	v2::PvfCheckStatement,
};
use scale_info::TypeInfo;
use sp_core::RuntimeDebug;
use sp_runtime::{
	traits::{AppVerify, One},
	DispatchResult, SaturatedConversion,
};
use sp_std::{cmp, convert::TryInto, mem, prelude::*};

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

pub use crate::Origin as ParachainOrigin;

#[cfg(feature = "runtime-benchmarks")]
pub(crate) mod benchmarking;

#[cfg(test)]
pub(crate) mod tests;

pub use pallet::*;

const LOG_TARGET: &str = "runtime::paras";

// the two key times necessary to track for every code replacement.
#[derive(Default, Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub struct ReplacementTimes<N> {
	/// The relay-chain block number that the code upgrade was expected to be activated.
	/// This is when the code change occurs from the para's perspective - after the
	/// first parablock included with a relay-parent with number >= this value.
	expected_at: N,
	/// The relay-chain block number at which the parablock activating the code upgrade was
	/// actually included. This means considered included and available, so this is the time at which
	/// that parablock enters the acceptance period in this fork of the relay-chain.
	activated_at: N,
}

/// Metadata used to track previous parachain validation code that we keep in
/// the state.
#[derive(Default, Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub struct ParaPastCodeMeta<N> {
	/// Block numbers where the code was expected to be replaced and where the code
	/// was actually replaced, respectively. The first is used to do accurate lookups
	/// of historic code in historic contexts, whereas the second is used to do
	/// pruning on an accurate timeframe. These can be used as indices
	/// into the `PastCodeHash` map along with the `ParaId` to fetch the code itself.
	upgrade_times: Vec<ReplacementTimes<N>>,
	/// Tracks the highest pruned code-replacement, if any. This is the `activated_at` value,
	/// not the `expected_at` value.
	last_pruned: Option<N>,
}

/// The possible states of a para, to take into account delayed lifecycle changes.
///
/// If the para is in a "transition state", it is expected that the parachain is
/// queued in the `ActionsQueue` to transition it into a stable state. Its lifecycle
/// state will be used to determine the state transition to apply to the para.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub enum ParaLifecycle {
	/// Para is new and is onboarding as a Parathread or Parachain.
	Onboarding,
	/// Para is a Parathread.
	Parathread,
	/// Para is a Parachain.
	Parachain,
	/// Para is a Parathread which is upgrading to a Parachain.
	UpgradingParathread,
	/// Para is a Parachain which is downgrading to a Parathread.
	DowngradingParachain,
	/// Parathread is queued to be offboarded.
	OffboardingParathread,
	/// Parachain is queued to be offboarded.
	OffboardingParachain,
}

impl ParaLifecycle {
	/// Returns true if parachain is currently onboarding. To learn if the
	/// parachain is onboarding as a parachain or parathread, look at the
	/// `UpcomingGenesis` storage item.
	pub fn is_onboarding(&self) -> bool {
		matches!(self, ParaLifecycle::Onboarding)
	}

	/// Returns true if para is in a stable state, i.e. it is currently
	/// a parachain or parathread, and not in any transition state.
	pub fn is_stable(&self) -> bool {
		matches!(self, ParaLifecycle::Parathread | ParaLifecycle::Parachain)
	}

	/// Returns true if para is currently treated as a parachain.
	/// This also includes transitioning states, so you may want to combine
	/// this check with `is_stable` if you specifically want `Paralifecycle::Parachain`.
	pub fn is_parachain(&self) -> bool {
		matches!(
			self,
			ParaLifecycle::Parachain |
				ParaLifecycle::DowngradingParachain |
				ParaLifecycle::OffboardingParachain
		)
	}

	/// Returns true if para is currently treated as a parathread.
	/// This also includes transitioning states, so you may want to combine
	/// this check with `is_stable` if you specifically want `Paralifecycle::Parathread`.
	pub fn is_parathread(&self) -> bool {
		matches!(
			self,
			ParaLifecycle::Parathread |
				ParaLifecycle::UpgradingParathread |
				ParaLifecycle::OffboardingParathread
		)
	}

	/// Returns true if para is currently offboarding.
	pub fn is_offboarding(&self) -> bool {
		matches!(self, ParaLifecycle::OffboardingParathread | ParaLifecycle::OffboardingParachain)
	}

	/// Returns true if para is in any transitionary state.
	pub fn is_transitioning(&self) -> bool {
		!Self::is_stable(self)
	}
}

impl<N: Ord + Copy + PartialEq> ParaPastCodeMeta<N> {
	// note a replacement has occurred at a given block number.
	pub(crate) fn note_replacement(&mut self, expected_at: N, activated_at: N) {
		self.upgrade_times.push(ReplacementTimes { expected_at, activated_at })
	}

	/// Returns `true` if the upgrade logs list is empty.
	fn is_empty(&self) -> bool {
		self.upgrade_times.is_empty()
	}

	// The block at which the most recently tracked code change occurred, from the perspective
	// of the para.
	#[cfg(test)]
	fn most_recent_change(&self) -> Option<N> {
		self.upgrade_times.last().map(|x| x.expected_at.clone())
	}

	// prunes all code upgrade logs occurring at or before `max`.
	// note that code replaced at `x` is the code used to validate all blocks before
	// `x`. Thus, `max` should be outside of the slashing window when this is invoked.
	//
	// Since we don't want to prune anything inside the acceptance period, and the parablock only
	// enters the acceptance period after being included, we prune based on the activation height of
	// the code change, not the expected height of the code change.
	//
	// returns an iterator of block numbers at which code was replaced, where the replaced
	// code should be now pruned, in ascending order.
	fn prune_up_to(&'_ mut self, max: N) -> impl Iterator<Item = N> + '_ {
		let to_prune = self.upgrade_times.iter().take_while(|t| t.activated_at <= max).count();
		let drained = if to_prune == 0 {
			// no-op prune.
			self.upgrade_times.drain(self.upgrade_times.len()..)
		} else {
			// if we are actually pruning something, update the `last_pruned` member.
			self.last_pruned = Some(self.upgrade_times[to_prune - 1].activated_at);
			self.upgrade_times.drain(..to_prune)
		};

		drained.map(|times| times.expected_at)
	}
}

/// Arguments for initializing a para.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct ParaGenesisArgs {
	/// The initial head data to use.
	pub genesis_head: HeadData,
	/// The initial validation code to use.
	pub validation_code: ValidationCode,
	/// True if parachain, false if parathread.
	pub parachain: bool,
}

/// This enum describes a reason why a particular PVF pre-checking vote was initiated. When the
/// PVF vote in question is concluded, this enum indicates what changes should be performed.
#[derive(Encode, Decode, TypeInfo)]
enum PvfCheckCause<BlockNumber> {
	/// PVF vote was initiated by the initial onboarding process of the given para.
	Onboarding(ParaId),
	/// PVF vote was initiated by signalling of an upgrade by the given para.
	Upgrade {
		/// The ID of the parachain that initiated or is waiting for the conclusion of pre-checking.
		id: ParaId,
		/// The relay-chain block number that was used as the relay-parent for the parablock that
		/// initiated the upgrade.
		relay_parent_number: BlockNumber,
	},
}

impl<BlockNumber> PvfCheckCause<BlockNumber> {
	/// Returns the ID of the para that initiated or subscribed to the pre-checking vote.
	fn para_id(&self) -> ParaId {
		match *self {
			PvfCheckCause::Onboarding(id) => id,
			PvfCheckCause::Upgrade { id, .. } => id,
		}
	}
}

/// Specifies what was the outcome of a PVF pre-checking vote.
#[derive(Copy, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
enum PvfCheckOutcome {
	Accepted,
	Rejected,
}

/// This struct describes the current state of an in-progress PVF pre-checking vote.
#[derive(Encode, Decode, TypeInfo)]
struct PvfCheckActiveVoteState<BlockNumber> {
	// The two following vectors have their length equal to the number of validators in the active
	// set. They start with all zeroes. A 1 is set at an index when the validator at the that index
	// makes a vote. Once a 1 is set for either of the vectors, that validator cannot vote anymore.
	// Since the active validator set changes each session, the bit vectors are reinitialized as
	// well: zeroed and resized so that each validator gets its own bit.
	votes_accept: BitVec<BitOrderLsb0, u8>,
	votes_reject: BitVec<BitOrderLsb0, u8>,

	/// The number of session changes this PVF vote has observed. Therefore, this number is
	/// increased at each session boundary. When created, it is initialized with 0.
	age: SessionIndex,
	/// The block number at which this PVF vote was created.
	created_at: BlockNumber,
	/// A list of causes for this PVF pre-checking. Has at least one.
	causes: Vec<PvfCheckCause<BlockNumber>>,
}

impl<BlockNumber> PvfCheckActiveVoteState<BlockNumber> {
	/// Returns a new instance of vote state, started at the specified block `now`, with the
	/// number of validators in the current session `n_validators` and the originating `cause`.
	fn new(now: BlockNumber, n_validators: usize, cause: PvfCheckCause<BlockNumber>) -> Self {
		let mut causes = Vec::with_capacity(1);
		causes.push(cause);
		Self {
			created_at: now,
			votes_accept: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
			votes_reject: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
			age: 0,
			causes,
		}
	}

	/// Resets all votes and resizes the votes vectors corresponding to the number of validators
	/// in the new session.
	fn reinitialize_ballots(&mut self, n_validators: usize) {
		let clear_and_resize = |v: &mut BitVec<_, _>| {
			v.clear();
			v.resize(n_validators, false);
		};
		clear_and_resize(&mut self.votes_accept);
		clear_and_resize(&mut self.votes_reject);
	}

	/// Returns `Some(true)` if the validator at the given index has already cast their vote within
	/// the ongoing session. Returns `None` in case the index is out of bounds.
	fn has_vote(&self, validator_index: usize) -> Option<bool> {
		let accept_vote = self.votes_accept.get(validator_index)?;
		let reject_vote = self.votes_reject.get(validator_index)?;
		Some(*accept_vote || *reject_vote)
	}

	/// Returns `None` if the quorum is not reached, or the direction of the decision.
	fn quorum(&self, n_validators: usize) -> Option<PvfCheckOutcome> {
		let q_threshold = primitives::v1::supermajority_threshold(n_validators);
		// NOTE: counting the reject votes is deliberately placed first. This is to err on the safe.
		if self.votes_reject.count_ones() >= q_threshold {
			Some(PvfCheckOutcome::Rejected)
		} else if self.votes_accept.count_ones() >= q_threshold {
			Some(PvfCheckOutcome::Accepted)
		} else {
			None
		}
	}
}

pub trait WeightInfo {
	fn force_set_current_code(c: u32) -> Weight;
	fn force_set_current_head(s: u32) -> Weight;
	fn force_schedule_code_upgrade(c: u32) -> Weight;
	fn force_note_new_head(s: u32) -> Weight;
	fn force_queue_action() -> Weight;
	fn add_trusted_validation_code(c: u32) -> Weight;
	fn poke_unused_validation_code() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn force_set_current_code(_c: u32) -> Weight {
		Weight::MAX
	}
	fn force_set_current_head(_s: u32) -> Weight {
		Weight::MAX
	}
	fn force_schedule_code_upgrade(_c: u32) -> Weight {
		Weight::MAX
	}
	fn force_note_new_head(_s: u32) -> Weight {
		Weight::MAX
	}
	fn force_queue_action() -> Weight {
		Weight::MAX
	}
	fn add_trusted_validation_code(_c: u32) -> Weight {
		Weight::MAX
	}
	fn poke_unused_validation_code() -> Weight {
		Weight::MAX
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use sp_runtime::transaction_validity::{
		InvalidTransaction, TransactionPriority, TransactionSource, TransactionValidity,
		ValidTransaction,
	};

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config
		+ configuration::Config
		+ shared::Config
		+ frame_system::offchain::SendTransactionTypes<Call<Self>>
	{
		type Event: From<Event> + IsType<<Self as frame_system::Config>::Event>;

		#[pallet::constant]
		type UnsignedPriority: Get<TransactionPriority>;

		type NextSessionRotation: EstimateNextSessionRotation<Self::BlockNumber>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event {
		/// Current code has been updated for a Para. `para_id`
		CurrentCodeUpdated(ParaId),
		/// Current head has been updated for a Para. `para_id`
		CurrentHeadUpdated(ParaId),
		/// A code upgrade has been scheduled for a Para. `para_id`
		CodeUpgradeScheduled(ParaId),
		/// A new head has been noted for a Para. `para_id`
		NewHeadNoted(ParaId),
		/// A para has been queued to execute pending actions. `para_id`
		ActionQueued(ParaId, SessionIndex),
		/// The given para either initiated or subscribed to a PVF check for the given validation
		/// code. `code_hash` `para_id`
		PvfCheckStarted(ValidationCodeHash, ParaId),
		/// The given validation code was rejected by the PVF pre-checking vote.
		/// `code_hash` `para_id`
		PvfCheckAccepted(ValidationCodeHash, ParaId),
		/// The given validation code was accepted by the PVF pre-checking vote.
		/// `code_hash` `para_id`
		PvfCheckRejected(ValidationCodeHash, ParaId),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Para is not registered in our system.
		NotRegistered,
		/// Para cannot be onboarded because it is already tracked by our system.
		CannotOnboard,
		/// Para cannot be offboarded at this time.
		CannotOffboard,
		/// Para cannot be upgraded to a parachain.
		CannotUpgrade,
		/// Para cannot be downgraded to a parathread.
		CannotDowngrade,
		/// The statement for PVF pre-checking is stale.
		PvfCheckStatementStale,
		/// The statement for PVF pre-checking is for a future session.
		PvfCheckStatementFuture,
		/// Claimed validator index is out of bounds.
		PvfCheckValidatorIndexOutOfBounds,
		/// The signature for the PVF pre-checking is invalid.
		PvfCheckInvalidSignature,
		/// The given validator already has cast a vote.
		PvfCheckDoubleVote,
		/// The given PVF does not exist at the moment of process a vote.
		PvfCheckSubjectInvalid,
		/// The PVF pre-checking statement cannot be included since the PVF pre-checking mechanism
		/// is disabled.
		PvfCheckDisabled,
	}

	/// All currently active PVF pre-checking votes.
	///
	/// Invariant:
	/// - There are no PVF pre-checking votes that exists in list but not in the set and vice versa.
	#[pallet::storage]
	pub(super) type PvfActiveVoteMap<T: Config> = StorageMap<
		_,
		Twox64Concat,
		ValidationCodeHash,
		PvfCheckActiveVoteState<T::BlockNumber>,
		OptionQuery,
	>;

	/// The list of all currently active PVF votes. Auxiliary to `PvfActiveVoteMap`.
	#[pallet::storage]
	pub(super) type PvfActiveVoteList<T: Config> =
		StorageValue<_, Vec<ValidationCodeHash>, ValueQuery>;

	/// All parachains. Ordered ascending by `ParaId`. Parathreads are not included.
	#[pallet::storage]
	#[pallet::getter(fn parachains)]
	pub(crate) type Parachains<T: Config> = StorageValue<_, Vec<ParaId>, ValueQuery>;

	/// The current lifecycle of a all known Para IDs.
	#[pallet::storage]
	pub(super) type ParaLifecycles<T: Config> = StorageMap<_, Twox64Concat, ParaId, ParaLifecycle>;

	/// The head-data of every registered para.
	#[pallet::storage]
	#[pallet::getter(fn para_head)]
	pub(super) type Heads<T: Config> = StorageMap<_, Twox64Concat, ParaId, HeadData>;

	/// The validation code hash of every live para.
	///
	/// Corresponding code can be retrieved with [`CodeByHash`].
	#[pallet::storage]
	#[pallet::getter(fn current_code_hash)]
	pub(super) type CurrentCodeHash<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, ValidationCodeHash>;

	/// Actual past code hash, indicated by the para id as well as the block number at which it
	/// became outdated.
	///
	/// Corresponding code can be retrieved with [`CodeByHash`].
	#[pallet::storage]
	pub(super) type PastCodeHash<T: Config> =
		StorageMap<_, Twox64Concat, (ParaId, T::BlockNumber), ValidationCodeHash>;

	/// Past code of parachains. The parachains themselves may not be registered anymore,
	/// but we also keep their code on-chain for the same amount of time as outdated code
	/// to keep it available for secondary checkers.
	#[pallet::storage]
	#[pallet::getter(fn past_code_meta)]
	pub(super) type PastCodeMeta<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, ParaPastCodeMeta<T::BlockNumber>, ValueQuery>;

	/// Which paras have past code that needs pruning and the relay-chain block at which the code was replaced.
	/// Note that this is the actual height of the included block, not the expected height at which the
	/// code upgrade would be applied, although they may be equal.
	/// This is to ensure the entire acceptance period is covered, not an offset acceptance period starting
	/// from the time at which the parachain perceives a code upgrade as having occurred.
	/// Multiple entries for a single para are permitted. Ordered ascending by block number.
	#[pallet::storage]
	pub(super) type PastCodePruning<T: Config> =
		StorageValue<_, Vec<(ParaId, T::BlockNumber)>, ValueQuery>;

	/// The block number at which the planned code change is expected for a para.
	/// The change will be applied after the first parablock for this ID included which executes
	/// in the context of a relay chain block with a number >= `expected_at`.
	#[pallet::storage]
	#[pallet::getter(fn future_code_upgrade_at)]
	pub(super) type FutureCodeUpgrades<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, T::BlockNumber>;

	/// The actual future code hash of a para.
	///
	/// Corresponding code can be retrieved with [`CodeByHash`].
	#[pallet::storage]
	pub(super) type FutureCodeHash<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, ValidationCodeHash>;

	/// This is used by the relay-chain to communicate to a parachain a go-ahead with in the upgrade procedure.
	///
	/// This value is absent when there are no upgrades scheduled or during the time the relay chain
	/// performs the checks. It is set at the first relay-chain block when the corresponding parachain
	/// can switch its upgrade function. As soon as the parachain's block is included, the value
	/// gets reset to `None`.
	///
	/// NOTE that this field is used by parachains via merkle storage proofs, therefore changing
	/// the format will require migration of parachains.
	#[pallet::storage]
	pub(super) type UpgradeGoAheadSignal<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, UpgradeGoAhead>;

	/// This is used by the relay-chain to communicate that there are restrictions for performing
	/// an upgrade for this parachain.
	///
	/// This may be a because the parachain waits for the upgrade cooldown to expire. Another
	/// potential use case is when we want to perform some maintenance (such as storage migration)
	/// we could restrict upgrades to make the process simpler.
	///
	/// NOTE that this field is used by parachains via merkle storage proofs, therefore changing
	/// the format will require migration of parachains.
	#[pallet::storage]
	pub(super) type UpgradeRestrictionSignal<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, UpgradeRestriction>;

	/// The list of parachains that are awaiting for their upgrade restriction to cooldown.
	///
	/// Ordered ascending by block number.
	#[pallet::storage]
	pub(super) type UpgradeCooldowns<T: Config> =
		StorageValue<_, Vec<(ParaId, T::BlockNumber)>, ValueQuery>;

	/// The list of upcoming code upgrades. Each item is a pair of which para performs a code
	/// upgrade and at which relay-chain block it is expected at.
	///
	/// Ordered ascending by block number.
	#[pallet::storage]
	pub(super) type UpcomingUpgrades<T: Config> =
		StorageValue<_, Vec<(ParaId, T::BlockNumber)>, ValueQuery>;

	/// The actions to perform during the start of a specific session index.
	#[pallet::storage]
	#[pallet::getter(fn actions_queue)]
	pub(super) type ActionsQueue<T: Config> =
		StorageMap<_, Twox64Concat, SessionIndex, Vec<ParaId>, ValueQuery>;

	/// Upcoming paras instantiation arguments.
	///
	/// NOTE that after PVF pre-checking is enabled the para genesis arg will have it's code set
	/// to empty. Instead, the code will be saved into the storage right away via `CodeByHash`.
	#[pallet::storage]
	pub(super) type UpcomingParasGenesis<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, ParaGenesisArgs>;

	/// The number of reference on the validation code in [`CodeByHash`] storage.
	#[pallet::storage]
	pub(super) type CodeByHashRefs<T: Config> =
		StorageMap<_, Identity, ValidationCodeHash, u32, ValueQuery>;

	/// Validation code stored by its hash.
	///
	/// This storage is consistent with [`FutureCodeHash`], [`CurrentCodeHash`] and
	/// [`PastCodeHash`].
	#[pallet::storage]
	#[pallet::getter(fn code_by_hash)]
	pub(super) type CodeByHash<T: Config> =
		StorageMap<_, Identity, ValidationCodeHash, ValidationCode>;

	#[pallet::genesis_config]
	pub struct GenesisConfig {
		pub paras: Vec<(ParaId, ParaGenesisArgs)>,
	}

	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			GenesisConfig { paras: Default::default() }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig {
		fn build(&self) {
			let mut parachains: Vec<_> = self
				.paras
				.iter()
				.filter(|(_, args)| args.parachain)
				.map(|&(ref id, _)| id)
				.cloned()
				.collect();

			parachains.sort();
			parachains.dedup();

			Parachains::<T>::put(&parachains);

			for (id, genesis_args) in &self.paras {
				let code_hash = genesis_args.validation_code.hash();
				<Pallet<T>>::increase_code_ref(&code_hash, &genesis_args.validation_code);
				<Pallet<T> as Store>::CurrentCodeHash::insert(&id, &code_hash);
				<Pallet<T> as Store>::Heads::insert(&id, &genesis_args.genesis_head);
				if genesis_args.parachain {
					ParaLifecycles::<T>::insert(&id, ParaLifecycle::Parachain);
				} else {
					ParaLifecycles::<T>::insert(&id, ParaLifecycle::Parathread);
				}
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Set the storage for the parachain validation code immediately.
		#[pallet::weight(<T as Config>::WeightInfo::force_set_current_code(new_code.0.len() as u32))]
		pub fn force_set_current_code(
			origin: OriginFor<T>,
			para: ParaId,
			new_code: ValidationCode,
		) -> DispatchResult {
			ensure_root(origin)?;
			let maybe_prior_code_hash = <Self as Store>::CurrentCodeHash::get(&para);
			let new_code_hash = new_code.hash();
			Self::increase_code_ref(&new_code_hash, &new_code);
			<Self as Store>::CurrentCodeHash::insert(&para, new_code_hash);

			let now = frame_system::Pallet::<T>::block_number();
			if let Some(prior_code_hash) = maybe_prior_code_hash {
				Self::note_past_code(para, now, now, prior_code_hash);
			} else {
				log::error!(
					target: LOG_TARGET,
					"Pallet paras storage is inconsistent, prior code not found {:?}",
					&para
				);
			}
			Self::deposit_event(Event::CurrentCodeUpdated(para));
			Ok(())
		}

		/// Set the storage for the current parachain head data immediately.
		#[pallet::weight(<T as Config>::WeightInfo::force_set_current_head(new_head.0.len() as u32))]
		pub fn force_set_current_head(
			origin: OriginFor<T>,
			para: ParaId,
			new_head: HeadData,
		) -> DispatchResult {
			ensure_root(origin)?;
			<Self as Store>::Heads::insert(&para, new_head);
			Self::deposit_event(Event::CurrentHeadUpdated(para));
			Ok(())
		}

		/// Schedule an upgrade as if it was scheduled in the given relay parent block.
		#[pallet::weight(<T as Config>::WeightInfo::force_schedule_code_upgrade(new_code.0.len() as u32))]
		pub fn force_schedule_code_upgrade(
			origin: OriginFor<T>,
			para: ParaId,
			new_code: ValidationCode,
			relay_parent_number: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			let config = configuration::Pallet::<T>::config();
			Self::schedule_code_upgrade(para, new_code, relay_parent_number, &config);
			Self::deposit_event(Event::CodeUpgradeScheduled(para));
			Ok(())
		}

		/// Note a new block head for para within the context of the current block.
		#[pallet::weight(<T as Config>::WeightInfo::force_note_new_head(new_head.0.len() as u32))]
		pub fn force_note_new_head(
			origin: OriginFor<T>,
			para: ParaId,
			new_head: HeadData,
		) -> DispatchResult {
			ensure_root(origin)?;
			let now = frame_system::Pallet::<T>::block_number();
			Self::note_new_head(para, new_head, now);
			Self::deposit_event(Event::NewHeadNoted(para));
			Ok(())
		}

		/// Put a parachain directly into the next session's action queue.
		/// We can't queue it any sooner than this without going into the
		/// initializer...
		#[pallet::weight(<T as Config>::WeightInfo::force_queue_action())]
		pub fn force_queue_action(origin: OriginFor<T>, para: ParaId) -> DispatchResult {
			ensure_root(origin)?;
			let next_session = shared::Pallet::<T>::session_index().saturating_add(One::one());
			ActionsQueue::<T>::mutate(next_session, |v| {
				if let Err(i) = v.binary_search(&para) {
					v.insert(i, para);
				}
			});
			Self::deposit_event(Event::ActionQueued(para, next_session));
			Ok(())
		}

		/// Adds the validation code to the storage.
		///
		/// The code will not be added if it is already present. Additionally, if PVF pre-checking
		/// is running for that code, it will be instantly accepted.
		///
		/// Otherwise, the code will be added into the storage. Note that the code will be added
		/// into storage with reference count 0. This is to account the fact that there are no users
		/// for this code yet. The caller will have to make sure that this code eventually gets
		/// used by some parachain or removed from the storage to avoid storage leaks. For the latter
		/// prefer to use the `poke_unused_validation_code` dispatchable to raw storage manipulation.
		///
		/// This function is mainly meant to be used for upgrading parachains that do not follow
		/// the go-ahead signal while the PVF pre-checking feature is enabled.
		#[pallet::weight(<T as Config>::WeightInfo::add_trusted_validation_code(validation_code.0.len() as u32))]
		pub fn add_trusted_validation_code(
			origin: OriginFor<T>,
			validation_code: ValidationCode,
		) -> DispatchResult {
			ensure_root(origin)?;
			let code_hash = validation_code.hash();

			if let Some(vote) = <Self as Store>::PvfActiveVoteMap::get(&code_hash) {
				// Remove the existing vote.
				PvfActiveVoteMap::<T>::remove(&code_hash);
				PvfActiveVoteList::<T>::mutate(|l| {
					if let Ok(i) = l.binary_search(&code_hash) {
						l.remove(i);
					}
				});

				let cfg = configuration::Pallet::<T>::config();
				Self::enact_pvf_accepted(
					<frame_system::Pallet<T>>::block_number(),
					&code_hash,
					&vote.causes,
					vote.age,
					&cfg,
				);
				return Ok(())
			}

			if <Self as Store>::CodeByHash::contains_key(&code_hash) {
				// There is no vote, but the code exists. Nothing to do here.
				return Ok(())
			}

			// At this point the code is unknown and there is no PVF pre-checking vote for it, so we
			// can just add the code into the storage.
			//
			// NOTE That we do not use `increase_code_ref` here, because the code is not yet used
			// by any parachain.
			<Self as Store>::CodeByHash::insert(code_hash, &validation_code);

			Ok(())
		}

		/// Remove the validation code from the storage iff the reference count is 0.
		///
		/// This is better than removing the storage directly, because it will not remove the code
		/// that was suddenly got used by some parachain while this dispatchable was pending
		/// dispatching.
		#[pallet::weight(<T as Config>::WeightInfo::poke_unused_validation_code())]
		pub fn poke_unused_validation_code(
			origin: OriginFor<T>,
			validation_code_hash: ValidationCodeHash,
		) -> DispatchResult {
			ensure_root(origin)?;
			if <Self as Store>::CodeByHashRefs::get(&validation_code_hash) == 0 {
				<Self as Store>::CodeByHash::remove(&validation_code_hash);
			}
			Ok(())
		}

		/// Includes a statement for a PVF pre-checking vote. Potentially, finalizes the vote and
		/// enacts the results if that was the last vote before achieving the supermajority.
		#[pallet::weight(Weight::MAX)]
		pub fn include_pvf_check_statement(
			origin: OriginFor<T>,
			stmt: PvfCheckStatement,
			signature: ValidatorSignature,
		) -> DispatchResult {
			ensure_none(origin)?;

			// Make sure that PVF pre-checking is enabled.
			ensure!(
				configuration::Pallet::<T>::config().pvf_checking_enabled,
				Error::<T>::PvfCheckDisabled,
			);

			let validators = shared::Pallet::<T>::active_validator_keys();
			let current_session = shared::Pallet::<T>::session_index();
			if stmt.session_index < current_session {
				return Err(Error::<T>::PvfCheckStatementStale.into())
			} else if stmt.session_index > current_session {
				return Err(Error::<T>::PvfCheckStatementFuture.into())
			}
			let validator_index = stmt.validator_index.0 as usize;
			let validator_public = validators
				.get(validator_index)
				.ok_or(Error::<T>::PvfCheckValidatorIndexOutOfBounds)?;

			let signing_payload = stmt.signing_payload();
			ensure!(
				signature.verify(&signing_payload[..], &validator_public),
				Error::<T>::PvfCheckInvalidSignature,
			);

			let mut active_vote = PvfActiveVoteMap::<T>::get(&stmt.subject)
				.ok_or(Error::<T>::PvfCheckSubjectInvalid)?;

			// Ensure that the validator submitting this statement hasn't voted already.
			ensure!(
				!active_vote
					.has_vote(validator_index)
					.ok_or(Error::<T>::PvfCheckValidatorIndexOutOfBounds)?,
				Error::<T>::PvfCheckDoubleVote,
			);

			// Finally, cast the vote and persist.
			if stmt.accept {
				active_vote.votes_accept.set(validator_index, true);
			} else {
				active_vote.votes_reject.set(validator_index, true);
			}

			if let Some(outcome) = active_vote.quorum(validators.len()) {
				// The supermajority quorum has been achieved.
				//
				// Remove the PVF vote from the active map and finalize the PVF checking according
				// to the outcome.
				PvfActiveVoteMap::<T>::remove(&stmt.subject);
				PvfActiveVoteList::<T>::mutate(|l| {
					if let Ok(i) = l.binary_search(&stmt.subject) {
						l.remove(i);
					}
				});
				match outcome {
					PvfCheckOutcome::Accepted => {
						let cfg = configuration::Pallet::<T>::config();
						Self::enact_pvf_accepted(
							<frame_system::Pallet<T>>::block_number(),
							&stmt.subject,
							&active_vote.causes,
							active_vote.age,
							&cfg,
						);
					},
					PvfCheckOutcome::Rejected => {
						Self::enact_pvf_rejected(&stmt.subject, active_vote.causes);
					},
				}
			} else {
				// No quorum has been achieved. So just store the updated state back into the
				// storage.
				PvfActiveVoteMap::<T>::insert(&stmt.subject, active_vote);
			}

			Ok(())
		}
	}

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;

		fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			let (stmt, signature) = match call {
				Call::include_pvf_check_statement { stmt, signature } => (stmt, signature),
				_ => return InvalidTransaction::Call.into(),
			};

			if !configuration::Pallet::<T>::config().pvf_checking_enabled {
				return InvalidTransaction::Custom(INVALID_TX_PVF_CHECK_DISABLED).into()
			}

			let current_session = shared::Pallet::<T>::session_index();
			if stmt.session_index < current_session {
				return InvalidTransaction::Stale.into()
			} else if stmt.session_index > current_session {
				return InvalidTransaction::Future.into()
			}

			let validator_index = stmt.validator_index.0 as usize;
			let validators = shared::Pallet::<T>::active_validator_keys();
			let validator_public = match validators.get(validator_index) {
				Some(pk) => pk,
				None => return InvalidTransaction::Custom(INVALID_TX_BAD_VALIDATOR_IDX).into(),
			};

			let signing_payload = stmt.signing_payload();
			if !signature.verify(&signing_payload[..], &validator_public) {
				return InvalidTransaction::BadProof.into()
			}

			let active_vote = match PvfActiveVoteMap::<T>::get(&stmt.subject) {
				Some(v) => v,
				None => return InvalidTransaction::Custom(INVALID_TX_BAD_SUBJECT).into(),
			};

			match active_vote.has_vote(validator_index) {
				Some(false) => (),
				Some(true) => return InvalidTransaction::Custom(INVALID_TX_DOUBLE_VOTE).into(),
				None => return InvalidTransaction::Custom(INVALID_TX_BAD_VALIDATOR_IDX).into(),
			}

			ValidTransaction::with_tag_prefix("PvfPreCheckingVote")
				.priority(T::UnsignedPriority::get())
				.longevity(
					TryInto::<u64>::try_into(
						T::NextSessionRotation::average_session_length() / 2u32.into(),
					)
					.unwrap_or(64_u64),
				)
				.and_provides((stmt.session_index, stmt.validator_index, stmt.subject))
				.propagate(true)
				.build()
		}

		fn pre_dispatch(_call: &Self::Call) -> Result<(), TransactionValidityError> {
			// Return `Ok` here meaning that as soon as the transaction got into the block, it will
			// always dispatched. This is OK, since the `include_pvf_check_statement` dispatchable
			// will perform the same checks anyway, so there is no point doing it here.
			//
			// On the other hand, if we did not provide the implementation, then the default
			// implementation would be used. The default implementation just delegates the
			// pre-dispatch validation to `validate_unsigned`.
			Ok(())
		}
	}
}

// custom transaction error codes
const INVALID_TX_BAD_VALIDATOR_IDX: u8 = 1;
const INVALID_TX_BAD_SUBJECT: u8 = 2;
const INVALID_TX_DOUBLE_VOTE: u8 = 3;
const INVALID_TX_PVF_CHECK_DISABLED: u8 = 4;

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the paras pallet.
	pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
		let weight = Self::prune_old_code(now);
		weight + Self::process_scheduled_upgrade_changes(now)
	}

	/// Called by the initializer to finalize the paras pallet.
	pub(crate) fn initializer_finalize(now: T::BlockNumber) {
		Self::process_scheduled_upgrade_cooldowns(now);
	}

	/// Called by the initializer to note that a new session has started.
	///
	/// Returns the list of outgoing paras from the actions queue.
	pub(crate) fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
	) -> Vec<ParaId> {
		let outgoing_paras = Self::apply_actions_queue(notification.session_index);
		Self::groom_ongoing_pvf_votes(&notification.new_config, notification.validators.len());
		outgoing_paras
	}

	/// The validation code of live para.
	pub(crate) fn current_code(para_id: &ParaId) -> Option<ValidationCode> {
		Self::current_code_hash(para_id).and_then(|code_hash| {
			let code = CodeByHash::<T>::get(&code_hash);
			if code.is_none() {
				log::error!(
					"Pallet paras storage is inconsistent, code not found for hash {}",
					code_hash,
				);
				debug_assert!(false, "inconsistent paras storages");
			}
			code
		})
	}

	// Apply all para actions queued for the given session index.
	//
	// The actions to take are based on the lifecycle of of the paras.
	//
	// The final state of any para after the actions queue should be as a
	// parachain, parathread, or not registered. (stable states)
	//
	// Returns the list of outgoing paras from the actions queue.
	fn apply_actions_queue(session: SessionIndex) -> Vec<ParaId> {
		let actions = ActionsQueue::<T>::take(session);
		let mut parachains = <Self as Store>::Parachains::get();
		let now = <frame_system::Pallet<T>>::block_number();
		let mut outgoing = Vec::new();

		for para in actions {
			let lifecycle = ParaLifecycles::<T>::get(&para);
			match lifecycle {
				None | Some(ParaLifecycle::Parathread) | Some(ParaLifecycle::Parachain) => { /* Nothing to do... */
				},
				Some(ParaLifecycle::Onboarding) => {
					if let Some(genesis_data) = <Self as Store>::UpcomingParasGenesis::take(&para) {
						if genesis_data.parachain {
							if let Err(i) = parachains.binary_search(&para) {
								parachains.insert(i, para);
							}
							ParaLifecycles::<T>::insert(&para, ParaLifecycle::Parachain);
						} else {
							ParaLifecycles::<T>::insert(&para, ParaLifecycle::Parathread);
						}

						// HACK: see the notice in `schedule_para_initialize`.
						//
						// Apparently, this is left over from a prior version of the runtime.
						// To handle this we just insert the code and link the current code hash
						// to it.
						if !genesis_data.validation_code.0.is_empty() {
							let code_hash = genesis_data.validation_code.hash();
							Self::increase_code_ref(&code_hash, &genesis_data.validation_code);
							<Self as Store>::CurrentCodeHash::insert(&para, code_hash);
						}

						<Self as Store>::Heads::insert(&para, genesis_data.genesis_head);
					}
				},
				// Upgrade a parathread to a parachain
				Some(ParaLifecycle::UpgradingParathread) => {
					if let Err(i) = parachains.binary_search(&para) {
						parachains.insert(i, para);
					}
					ParaLifecycles::<T>::insert(&para, ParaLifecycle::Parachain);
				},
				// Downgrade a parachain to a parathread
				Some(ParaLifecycle::DowngradingParachain) => {
					if let Ok(i) = parachains.binary_search(&para) {
						parachains.remove(i);
					}
					ParaLifecycles::<T>::insert(&para, ParaLifecycle::Parathread);
				},
				// Offboard a parathread or parachain from the system
				Some(ParaLifecycle::OffboardingParachain) |
				Some(ParaLifecycle::OffboardingParathread) => {
					if let Ok(i) = parachains.binary_search(&para) {
						parachains.remove(i);
					}

					<Self as Store>::Heads::remove(&para);
					<Self as Store>::FutureCodeUpgrades::remove(&para);
					<Self as Store>::UpgradeGoAheadSignal::remove(&para);
					<Self as Store>::UpgradeRestrictionSignal::remove(&para);
					ParaLifecycles::<T>::remove(&para);
					let removed_future_code_hash = <Self as Store>::FutureCodeHash::take(&para);
					if let Some(removed_future_code_hash) = removed_future_code_hash {
						Self::decrease_code_ref(&removed_future_code_hash);
					}

					let removed_code_hash = <Self as Store>::CurrentCodeHash::take(&para);
					if let Some(removed_code_hash) = removed_code_hash {
						Self::note_past_code(para, now, now, removed_code_hash);
					}

					outgoing.push(para);
				},
			}
		}

		if !outgoing.is_empty() {
			// Filter offboarded parachains from the upcoming upgrades and upgrade cooldowns list.
			//
			// We do it after the offboarding to get away with only a single read/write per list.
			//
			// NOTE both of those iterates over the list and the outgoing. We do not expect either
			//      of these to be large. Thus should be fine.
			<Self as Store>::UpcomingUpgrades::mutate(|upcoming_upgrades| {
				*upcoming_upgrades = mem::take(upcoming_upgrades)
					.into_iter()
					.filter(|&(ref para, _)| !outgoing.contains(para))
					.collect();
			});
			<Self as Store>::UpgradeCooldowns::mutate(|upgrade_cooldowns| {
				*upgrade_cooldowns = mem::take(upgrade_cooldowns)
					.into_iter()
					.filter(|&(ref para, _)| !outgoing.contains(para))
					.collect();
			});
		}

		// Place the new parachains set in storage.
		<Self as Store>::Parachains::set(parachains);

		return outgoing
	}

	// note replacement of the code of para with given `id`, which occured in the
	// context of the given relay-chain block number. provide the replaced code.
	//
	// `at` for para-triggered replacement is the block number of the relay-chain
	// block in whose context the parablock was executed
	// (i.e. number of `relay_parent` in the receipt)
	fn note_past_code(
		id: ParaId,
		at: T::BlockNumber,
		now: T::BlockNumber,
		old_code_hash: ValidationCodeHash,
	) -> Weight {
		<Self as Store>::PastCodeMeta::mutate(&id, |past_meta| {
			past_meta.note_replacement(at, now);
		});

		<Self as Store>::PastCodeHash::insert(&(id, at), old_code_hash);

		// Schedule pruning for this past-code to be removed as soon as it
		// exits the slashing window.
		<Self as Store>::PastCodePruning::mutate(|pruning| {
			let insert_idx =
				pruning.binary_search_by_key(&now, |&(_, b)| b).unwrap_or_else(|idx| idx);
			pruning.insert(insert_idx, (id, now));
		});

		T::DbWeight::get().reads_writes(2, 3)
	}

	// looks at old code metadata, compares them to the current acceptance window, and prunes those
	// that are too old.
	fn prune_old_code(now: T::BlockNumber) -> Weight {
		let config = configuration::Pallet::<T>::config();
		let code_retention_period = config.code_retention_period;
		if now <= code_retention_period {
			let weight = T::DbWeight::get().reads_writes(1, 0);
			return weight
		}

		// The height of any changes we no longer should keep around.
		let pruning_height = now - (code_retention_period + One::one());

		let pruning_tasks_done = <Self as Store>::PastCodePruning::mutate(
			|pruning_tasks: &mut Vec<(_, T::BlockNumber)>| {
				let (pruning_tasks_done, pruning_tasks_to_do) = {
					// find all past code that has just exited the pruning window.
					let up_to_idx =
						pruning_tasks.iter().take_while(|&(_, at)| at <= &pruning_height).count();
					(up_to_idx, pruning_tasks.drain(..up_to_idx))
				};

				for (para_id, _) in pruning_tasks_to_do {
					let full_deactivate = <Self as Store>::PastCodeMeta::mutate(&para_id, |meta| {
						for pruned_repl_at in meta.prune_up_to(pruning_height) {
							let removed_code_hash =
								<Self as Store>::PastCodeHash::take(&(para_id, pruned_repl_at));

							if let Some(removed_code_hash) = removed_code_hash {
								Self::decrease_code_ref(&removed_code_hash);
							} else {
								log::warn!(
									target: LOG_TARGET,
									"Missing code for removed hash {:?}",
									removed_code_hash,
								);
							}
						}

						meta.is_empty() && Self::para_head(&para_id).is_none()
					});

					// This parachain has been removed and now the vestigial code
					// has been removed from the state. clean up meta as well.
					if full_deactivate {
						<Self as Store>::PastCodeMeta::remove(&para_id);
					}
				}

				pruning_tasks_done as u64
			},
		);

		// 1 read for the meta for each pruning task, 1 read for the config
		// 2 writes: updating the meta and pruning the code
		T::DbWeight::get().reads_writes(1 + pruning_tasks_done, 2 * pruning_tasks_done)
	}

	/// Process the timers related to upgrades. Specifically, the upgrade go ahead signals toggle
	/// and the upgrade cooldown restrictions. However, this function does not actually unset
	/// the upgrade restriction, that will happen in the `initializer_finalize` function. However,
	/// this function does count the number of cooldown timers expired so that we can reserve weight
	/// for the `initializer_finalize` function.
	fn process_scheduled_upgrade_changes(now: T::BlockNumber) -> Weight {
		// account weight for `UpcomingUpgrades::mutate`.
		let mut weight = T::DbWeight::get().reads_writes(1, 1);
		let upgrades_signaled = <Self as Store>::UpcomingUpgrades::mutate(
			|upcoming_upgrades: &mut Vec<(ParaId, T::BlockNumber)>| {
				let num = upcoming_upgrades.iter().take_while(|&(_, at)| at <= &now).count();
				for (para, _) in upcoming_upgrades.drain(..num) {
					<Self as Store>::UpgradeGoAheadSignal::insert(&para, UpgradeGoAhead::GoAhead);
				}
				num
			},
		);
		weight += T::DbWeight::get().writes(upgrades_signaled as u64);

		// account weight for `UpgradeCooldowns::get`.
		weight += T::DbWeight::get().reads(1);
		let cooldowns_expired = <Self as Store>::UpgradeCooldowns::get()
			.iter()
			.take_while(|&(_, at)| at <= &now)
			.count();

		// reserve weight for `initializer_finalize`:
		// - 1 read and 1 write for `UpgradeCooldowns::mutate`.
		// - 1 write per expired cooldown.
		weight += T::DbWeight::get().reads_writes(1, 1);
		weight += T::DbWeight::get().reads(cooldowns_expired as u64);

		weight
	}

	/// Actually perform unsetting the expired upgrade restrictions.
	///
	/// See `process_scheduled_upgrade_changes` for more details.
	fn process_scheduled_upgrade_cooldowns(now: T::BlockNumber) {
		<Self as Store>::UpgradeCooldowns::mutate(
			|upgrade_cooldowns: &mut Vec<(ParaId, T::BlockNumber)>| {
				for &(para, _) in upgrade_cooldowns.iter().take_while(|&(_, at)| at <= &now) {
					<Self as Store>::UpgradeRestrictionSignal::remove(&para);
				}
			},
		);
	}

	/// Goes over all PVF votes in progress, reinitializes ballots, increments ages and prunes the
	/// active votes that reached their time-to-live.
	fn groom_ongoing_pvf_votes(
		cfg: &configuration::HostConfiguration<T::BlockNumber>,
		new_n_validators: usize,
	) -> Weight {
		let mut weight = T::DbWeight::get().reads(1);

		let potentially_active_votes = PvfActiveVoteList::<T>::get();

		// Initially empty list which contains all the PVF active votes that made it through this
		// session change.
		//
		// **Ordered** as well as `PvfActiveVoteList`.
		let mut actually_active_votes = Vec::with_capacity(potentially_active_votes.len());

		for vote_subject in potentially_active_votes {
			let mut vote_state = match PvfActiveVoteMap::<T>::take(&vote_subject) {
				Some(v) => v,
				None => {
					// This branch should never be reached. This is due to the fact that the set of
					// `PvfActiveVoteMap`'s keys is always equal to the set of items found in
					// `PvfActiveVoteList`.
					log::warn!(
						target: LOG_TARGET,
						"The PvfActiveVoteMap is out of sync with PvfActiveVoteList!",
					);
					debug_assert!(false);
					continue
				},
			};

			vote_state.age += 1;
			if vote_state.age < cfg.pvf_voting_ttl {
				weight += T::DbWeight::get().writes(1);
				vote_state.reinitialize_ballots(new_n_validators);
				PvfActiveVoteMap::<T>::insert(&vote_subject, vote_state);

				// push maintaining the original order.
				actually_active_votes.push(vote_subject);
			} else {
				// TTL is reached. Reject.
				weight += Self::enact_pvf_rejected(&vote_subject, vote_state.causes);
			}
		}

		weight += T::DbWeight::get().writes(1);
		PvfActiveVoteList::<T>::put(actually_active_votes);

		weight
	}

	fn enact_pvf_accepted(
		now: T::BlockNumber,
		code_hash: &ValidationCodeHash,
		causes: &[PvfCheckCause<T::BlockNumber>],
		sessions_observed: SessionIndex,
		cfg: &configuration::HostConfiguration<T::BlockNumber>,
	) -> Weight {
		let mut weight = 0;
		for cause in causes {
			weight += T::DbWeight::get().reads_writes(3, 2);
			Self::deposit_event(Event::PvfCheckAccepted(*code_hash, cause.para_id()));

			match cause {
				PvfCheckCause::Onboarding(id) => {
					weight += Self::proceed_with_onboarding(*id, sessions_observed);
				},
				PvfCheckCause::Upgrade { id, relay_parent_number } => {
					weight +=
						Self::proceed_with_upgrade(*id, code_hash, now, *relay_parent_number, cfg);
				},
			}
		}
		weight
	}

	fn proceed_with_onboarding(id: ParaId, sessions_observed: SessionIndex) -> Weight {
		let weight = T::DbWeight::get().reads_writes(2, 1);

		// we should onboard only after `SESSION_DELAY` sessions but we should take
		// into account the number of sessions the PVF pre-checking occupied.
		//
		// we cannot onboard at the current session, so it must be at least one
		// session ahead.
		let onboard_at: SessionIndex = shared::Pallet::<T>::session_index() +
			cmp::max(shared::SESSION_DELAY.saturating_sub(sessions_observed), 1);

		ActionsQueue::<T>::mutate(onboard_at, |v| {
			if let Err(i) = v.binary_search(&id) {
				v.insert(i, id);
			}
		});

		weight
	}

	fn proceed_with_upgrade(
		id: ParaId,
		code_hash: &ValidationCodeHash,
		now: T::BlockNumber,
		relay_parent_number: T::BlockNumber,
		cfg: &configuration::HostConfiguration<T::BlockNumber>,
	) -> Weight {
		let mut weight = 0;

		// Compute the relay-chain block number starting at which the code upgrade is ready to be
		// applied.
		//
		// The first parablock that has a relay-parent higher or at the same height of `expected_at`
		// will trigger the code upgrade. The parablock that comes after that will be validated
		// against the new validation code.
		//
		// Here we are trying to choose the block number that will have `validation_upgrade_delay`
		// blocks from the relay-parent of the block that schedule code upgrade but no less than
		// `minimum_validation_upgrade_delay`. We want this delay out of caution so that when
		// the last vote for pre-checking comes the parachain will have some time until the upgrade
		// finally takes place.
		let expected_at = cmp::max(
			relay_parent_number + cfg.validation_upgrade_delay,
			now + cfg.minimum_validation_upgrade_delay,
		);

		weight += T::DbWeight::get().reads_writes(1, 4);
		FutureCodeUpgrades::<T>::insert(&id, expected_at);

		<Self as Store>::UpcomingUpgrades::mutate(|upcoming_upgrades| {
			let insert_idx = upcoming_upgrades
				.binary_search_by_key(&expected_at, |&(_, b)| b)
				.unwrap_or_else(|idx| idx);
			upcoming_upgrades.insert(insert_idx, (id, expected_at));
		});

		let expected_at = expected_at.saturated_into();
		let log = ConsensusLog::ParaScheduleUpgradeCode(id, *code_hash, expected_at);
		<frame_system::Pallet<T>>::deposit_log(log.into());

		weight
	}

	fn enact_pvf_rejected(
		code_hash: &ValidationCodeHash,
		causes: Vec<PvfCheckCause<T::BlockNumber>>,
	) -> Weight {
		let mut weight = 0;

		for cause in causes {
			// Whenever PVF pre-checking is started or a new cause is added to it, the RC is bumped.
			// Now we need to unbump it.
			weight += Self::decrease_code_ref(code_hash);

			weight += T::DbWeight::get().reads_writes(3, 2);
			Self::deposit_event(Event::PvfCheckRejected(*code_hash, cause.para_id()));

			match cause {
				PvfCheckCause::Onboarding(id) => {
					// Here we need to undo everything that was done during `schedule_para_initialize`.
					// Essentially, the logic is similar to offboarding, with exception that before
					// actual onboarding the parachain did not have a chance to reach to upgrades.
					// Therefore we can skip all the upgrade related storage items here.
					weight += T::DbWeight::get().writes(3);
					UpcomingParasGenesis::<T>::remove(&id);
					CurrentCodeHash::<T>::remove(&id);
					ParaLifecycles::<T>::remove(&id);
				},
				PvfCheckCause::Upgrade { id, .. } => {
					weight += T::DbWeight::get().writes(2);
					UpgradeGoAheadSignal::<T>::insert(&id, UpgradeGoAhead::Abort);
					FutureCodeHash::<T>::remove(&id);
				},
			}
		}

		weight
	}

	/// Verify that `schedule_para_initialize` can be called successfully.
	///
	/// Returns false if para is already registered in the system.
	pub fn can_schedule_para_initialize(id: &ParaId) -> bool {
		ParaLifecycles::<T>::get(id).is_none()
	}

	/// Schedule a para to be initialized. If the validation code is not already stored in the
	/// code storage, then a PVF pre-checking process will be initiated.
	///
	/// Only after the PVF pre-checking succeeds can the para be onboarded. Note, that calling this
	/// does not guarantee that the parachain will eventually be onboarded. This can happen in case
	/// the PVF does not pass PVF pre-checking.
	///
	/// The Para ID should be not activated in this pallet. The validation code supplied in
	/// `genesis_data` should not be empty. If those conditions are not met, then the para cannot
	/// be onboarded.
	pub(crate) fn schedule_para_initialize(
		id: ParaId,
		mut genesis_data: ParaGenesisArgs,
	) -> DispatchResult {
		// Make sure parachain isn't already in our system and that the onboarding parameters are
		// valid.
		ensure!(Self::can_schedule_para_initialize(&id), Error::<T>::CannotOnboard);
		ensure!(!genesis_data.validation_code.0.is_empty(), Error::<T>::CannotOnboard);
		ParaLifecycles::<T>::insert(&id, ParaLifecycle::Onboarding);

		// HACK: here we are doing something nasty.
		//
		// In order to fix the [soaking issue] we insert the code eagerly here. When the onboarding
		// is finally enacted, we do not need to insert the code anymore. Therefore, there is no
		// reason for the validation code to be copied into the `ParaGenesisArgs`. We also do not
		// want to risk it by copying the validation code needlessly to not risk adding more
		// memory pressure.
		//
		// That said, we also want to preserve `ParaGenesisArgs` as it is, for now. There are two
		// reasons:
		//
		// - Doing it within the context of the PR that introduces this change is undesirable, since
		//   it is already a big change, and that change would require a migration. Moreover, if we
		//   run the new version of the runtime, there will be less things to worry about during
		//   the eventual proper migration.
		//
		// - This data type already is used for generating genesis, and changing it will probably
		//   introduce some unnecessary burden.
		//
		// So instead of going through it right now, we will do something sneaky. Specifically:
		//
		// - Insert the `CurrentCodeHash` now, instead during the onboarding. That would allow to
		//   get rid of hashing of the validation code when onboarding.
		//
		// - Replace `validation_code` with a sentinel value: an empty vector. This should be fine
		//   as long we do not allow registering parachains with empty code. At the moment of writing
		//   this should already be the case.
		//
		// - Empty value is treated as the current code is already inserted during the onboarding.
		//
		// This is only an intermediate solution and should be fixed in foreseable future.
		//
		// [soaking issue]: https://github.com/paritytech/polkadot/issues/3918
		let validation_code =
			mem::replace(&mut genesis_data.validation_code, ValidationCode(Vec::new()));
		UpcomingParasGenesis::<T>::insert(&id, genesis_data);
		let validation_code_hash = validation_code.hash();
		<Self as Store>::CurrentCodeHash::insert(&id, validation_code_hash);

		let cfg = configuration::Pallet::<T>::config();
		Self::kick_off_pvf_check(
			PvfCheckCause::Onboarding(id),
			validation_code_hash,
			validation_code,
			&cfg,
		);

		Ok(())
	}

	/// Schedule a para to be cleaned up at the start of the next session.
	///
	/// Will return error if either is true:
	///
	/// - para is not a stable parachain or parathread (i.e. [`ParaLifecycle::is_stable`] is `false`)
	/// - para has a pending upgrade.
	///
	/// No-op if para is not registered at all.
	pub(crate) fn schedule_para_cleanup(id: ParaId) -> DispatchResult {
		// Disallow offboarding in case there is an upcoming upgrade.
		//
		// This is not a fundamential limitation but rather simplification: it allows us to get
		// away without introducing additional logic for pruning and, more importantly, enacting
		// ongoing PVF pre-checking votes. It also removes some nasty edge cases.
		//
		// This implicitly assumes that the given para exists, i.e. it's lifecycle != None.
		if FutureCodeHash::<T>::contains_key(&id) {
			return Err(Error::<T>::CannotOffboard.into())
		}

		let lifecycle = ParaLifecycles::<T>::get(&id);
		match lifecycle {
			// If para is not registered, nothing to do!
			None => return Ok(()),
			Some(ParaLifecycle::Parathread) => {
				ParaLifecycles::<T>::insert(&id, ParaLifecycle::OffboardingParathread);
			},
			Some(ParaLifecycle::Parachain) => {
				ParaLifecycles::<T>::insert(&id, ParaLifecycle::OffboardingParachain);
			},
			_ => return Err(Error::<T>::CannotOffboard)?,
		}

		let scheduled_session = Self::scheduled_session();
		ActionsQueue::<T>::mutate(scheduled_session, |v| {
			if let Err(i) = v.binary_search(&id) {
				v.insert(i, id);
			}
		});

		Ok(())
	}

	/// Schedule a parathread to be upgraded to a parachain.
	///
	/// Will return error if `ParaLifecycle` is not `Parathread`.
	pub(crate) fn schedule_parathread_upgrade(id: ParaId) -> DispatchResult {
		let scheduled_session = Self::scheduled_session();
		let lifecycle = ParaLifecycles::<T>::get(&id).ok_or(Error::<T>::NotRegistered)?;

		ensure!(lifecycle == ParaLifecycle::Parathread, Error::<T>::CannotUpgrade);

		ParaLifecycles::<T>::insert(&id, ParaLifecycle::UpgradingParathread);
		ActionsQueue::<T>::mutate(scheduled_session, |v| {
			if let Err(i) = v.binary_search(&id) {
				v.insert(i, id);
			}
		});

		Ok(())
	}

	/// Schedule a parachain to be downgraded to a parathread.
	///
	/// Noop if `ParaLifecycle` is not `Parachain`.
	pub(crate) fn schedule_parachain_downgrade(id: ParaId) -> DispatchResult {
		let scheduled_session = Self::scheduled_session();
		let lifecycle = ParaLifecycles::<T>::get(&id).ok_or(Error::<T>::NotRegistered)?;

		ensure!(lifecycle == ParaLifecycle::Parachain, Error::<T>::CannotDowngrade);

		ParaLifecycles::<T>::insert(&id, ParaLifecycle::DowngradingParachain);
		ActionsQueue::<T>::mutate(scheduled_session, |v| {
			if let Err(i) = v.binary_search(&id) {
				v.insert(i, id);
			}
		});

		Ok(())
	}

	/// Schedule a future code upgrade of the given parachain.
	///
	/// If the new code is not known, then the PVF pre-checking will be started for that validation
	/// code. In case the validation code does not pass the PVF pre-checking process, the
	/// upgrade will be aborted.
	///
	/// Only after the code is approved by the process, the upgrade can be scheduled. Specifically,
	/// the relay-chain block number will be determined at which the upgrade will take place. We
	/// call that block `expected_at`.
	///
	/// Once the candidate with the relay-parent >= `expected_at` is enacted, the new validation code
	/// will be applied. Therefore, the new code will be used to validate the next candidate.
	///
	/// The new code should not be equal to the current one, otherwise the upgrade will be aborted.
	/// If there is already a scheduled code upgrade for the para, this is a no-op.
	pub(crate) fn schedule_code_upgrade(
		id: ParaId,
		new_code: ValidationCode,
		relay_parent_number: T::BlockNumber,
		cfg: &configuration::HostConfiguration<T::BlockNumber>,
	) -> Weight {
		let mut weight = T::DbWeight::get().reads(1);

		// Enacting this should be prevented by the `can_schedule_upgrade`
		if FutureCodeHash::<T>::contains_key(&id) {
			// This branch should never be reached. Signalling an upgrade is disallowed for a para
			// that already has one upgrade scheduled.
			//
			// Any candidate that attempts to do that should be rejected by
			// `can_upgrade_validation_code`.
			//
			// NOTE: we cannot set `UpgradeGoAheadSignal` signal here since this will be reset by
			//       the following call `note_new_head`
			log::warn!(target: LOG_TARGET, "ended up scheduling an upgrade while one is pending",);
			return weight
		}

		let code_hash = new_code.hash();

		// para signals an update to the same code? This does not make a lot of sense, so abort the
		// process right away.
		//
		// We do not want to allow this since it will mess with the code reference counting.
		weight += T::DbWeight::get().reads(1);
		if CurrentCodeHash::<T>::get(&id) == Some(code_hash) {
			// NOTE: we cannot set `UpgradeGoAheadSignal` signal here since this will be reset by
			//       the following call `note_new_head`
			log::warn!(
				target: LOG_TARGET,
				"para tried to upgrade to the same code. Abort the upgrade",
			);
			return weight
		}

		// This is the start of the upgrade process. Prevent any further attempts at upgrading.
		weight += T::DbWeight::get().writes(2);
		FutureCodeHash::<T>::insert(&id, &code_hash);
		UpgradeRestrictionSignal::<T>::insert(&id, UpgradeRestriction::Present);

		weight += T::DbWeight::get().reads_writes(1, 1);
		let next_possible_upgrade_at = relay_parent_number + cfg.validation_upgrade_cooldown;
		<Self as Store>::UpgradeCooldowns::mutate(|upgrade_cooldowns| {
			let insert_idx = upgrade_cooldowns
				.binary_search_by_key(&next_possible_upgrade_at, |&(_, b)| b)
				.unwrap_or_else(|idx| idx);
			upgrade_cooldowns.insert(insert_idx, (id, next_possible_upgrade_at));
		});

		weight += Self::kick_off_pvf_check(
			PvfCheckCause::Upgrade { id, relay_parent_number },
			code_hash,
			new_code,
			cfg,
		);

		weight
	}

	/// Makes sure that the given code hash has passed pre-checking.
	///
	/// If the given code hash has already passed pre-checking, then the approval happens
	/// immediately. Similarly, if the pre-checking is turned off, the update is scheduled immediately
	/// as well. In this case, the behavior is similar to the previous, i.e. the upgrade sequence
	/// is purely time-based.
	///
	/// If the code is unknown, but the pre-checking for that PVF is already running then we perform
	/// "coalescing". We save the cause for this PVF pre-check request and just add it to the
	/// existing active PVF vote.
	///
	/// And finally, if the code is unknown and pre-checking is not running, we start the
	/// pre-checking process anew.
	///
	/// Unconditionally increases the reference count for the passed `code`.
	fn kick_off_pvf_check(
		cause: PvfCheckCause<T::BlockNumber>,
		code_hash: ValidationCodeHash,
		code: ValidationCode,
		cfg: &configuration::HostConfiguration<T::BlockNumber>,
	) -> Weight {
		let mut weight = 0;

		weight += T::DbWeight::get().reads_writes(3, 2);
		Self::deposit_event(Event::PvfCheckStarted(code_hash, cause.para_id()));

		weight += T::DbWeight::get().reads(1);
		match PvfActiveVoteMap::<T>::get(&code_hash) {
			None => {
				// We deliberately are using `CodeByHash` here instead of the `CodeByHashRefs`. This
				// is because the code may have been added by `add_trusted_validation_code`.
				let known_code = CodeByHash::<T>::contains_key(&code_hash);
				weight += T::DbWeight::get().reads(1);

				if !cfg.pvf_checking_enabled || known_code {
					// Either:
					// - the code is known and there is no active PVF vote for it meaning it is
					//   already checked, or
					// - the PVF checking is diabled
					// In any case: fast track the PVF checking into the accepted state
					weight += T::DbWeight::get().reads(1);
					let now = <frame_system::Pallet<T>>::block_number();
					weight += Self::enact_pvf_accepted(now, &code_hash, &[cause], 0, cfg);
				} else {
					// PVF is not being pre-checked and it is not known. Start a new pre-checking
					// process.
					weight += T::DbWeight::get().reads_writes(3, 2);
					let now = <frame_system::Pallet<T>>::block_number();
					let n_validators = shared::Pallet::<T>::active_validator_keys().len();
					PvfActiveVoteMap::<T>::insert(
						&code_hash,
						PvfCheckActiveVoteState::new(now, n_validators, cause),
					);
					PvfActiveVoteList::<T>::mutate(|l| {
						if let Err(idx) = l.binary_search(&code_hash) {
							l.insert(idx, code_hash);
						}
					});
				}
			},
			Some(mut vote_state) => {
				// Coalescing: the PVF is already being pre-checked so we just need to piggy back
				// on it.
				weight += T::DbWeight::get().writes(1);
				vote_state.causes.push(cause);
				PvfActiveVoteMap::<T>::insert(&code_hash, vote_state);
			},
		}

		// We increase the code RC here in any case. Intuitively the parachain that requested this
		// action is now a user of that PVF.
		//
		// If the result of the pre-checking is reject, then we would decrease the RC for each cause,
		// including the current.
		//
		// If the result of the pre-checking is accept, then we do nothing to the RC because the PVF
		// will continue be used by the same users.
		//
		// If the PVF was fast-tracked (i.e. there is already non zero RC) and there is no
		// pre-checking, we also do not change the RC then.
		weight += Self::increase_code_ref(&code_hash, &code);

		weight
	}

	/// Note that a para has progressed to a new head, where the new head was executed in the context
	/// of a relay-chain block with given number. This will apply pending code upgrades based
	/// on the relay-parent block number provided.
	pub(crate) fn note_new_head(
		id: ParaId,
		new_head: HeadData,
		execution_context: T::BlockNumber,
	) -> Weight {
		Heads::<T>::insert(&id, new_head);

		if let Some(expected_at) = <Self as Store>::FutureCodeUpgrades::get(&id) {
			if expected_at <= execution_context {
				<Self as Store>::FutureCodeUpgrades::remove(&id);
				<Self as Store>::UpgradeGoAheadSignal::remove(&id);

				// Both should always be `Some` in this case, since a code upgrade is scheduled.
				let new_code_hash = if let Some(new_code_hash) = FutureCodeHash::<T>::take(&id) {
					new_code_hash
				} else {
					log::error!(target: LOG_TARGET, "Missing future code hash for {:?}", &id);
					return T::DbWeight::get().reads_writes(3, 1 + 3)
				};
				let maybe_prior_code_hash = CurrentCodeHash::<T>::get(&id);
				CurrentCodeHash::<T>::insert(&id, &new_code_hash);

				let log = ConsensusLog::ParaUpgradeCode(id, new_code_hash);
				<frame_system::Pallet<T>>::deposit_log(log.into());

				// `now` is only used for registering pruning as part of `fn note_past_code`
				let now = <frame_system::Pallet<T>>::block_number();

				let weight = if let Some(prior_code_hash) = maybe_prior_code_hash {
					Self::note_past_code(id, expected_at, now, prior_code_hash)
				} else {
					log::error!(target: LOG_TARGET, "Missing prior code hash for para {:?}", &id);
					0 as Weight
				};

				// add 1 to writes due to heads update.
				weight + T::DbWeight::get().reads_writes(3, 1 + 3)
			} else {
				T::DbWeight::get().reads_writes(1, 1 + 0)
			}
		} else {
			// This means there is no upgrade scheduled.
			//
			// In case the upgrade was aborted by the relay-chain we should reset
			// the `Abort` signal.
			UpgradeGoAheadSignal::<T>::remove(&id);
			T::DbWeight::get().reads_writes(1, 2)
		}
	}

	/// Returns the list of PVFs (aka validation code) that require casting a vote by a validator in
	/// the active validator set.
	pub(crate) fn pvfs_require_precheck() -> Vec<ValidationCodeHash> {
		PvfActiveVoteList::<T>::get()
	}

	/// Submits a given PVF check statement with corresponding signature as an unsigned transaction
	/// into the memory pool. Ultimately, that disseminates the transaction accross the network.
	///
	/// This function expects an offchain context and cannot be callable from the on-chain logic.
	///
	/// The signature assumed to pertain to `stmt`.
	pub(crate) fn submit_pvf_check_statement(
		stmt: PvfCheckStatement,
		signature: ValidatorSignature,
	) {
		use frame_system::offchain::SubmitTransaction;

		if let Err(e) = SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(
			Call::include_pvf_check_statement { stmt, signature }.into(),
		) {
			log::error!(target: LOG_TARGET, "Error submitting pvf check statement: {:?}", e,);
		}
	}

	/// Returns the current lifecycle state of the para.
	pub fn lifecycle(id: ParaId) -> Option<ParaLifecycle> {
		ParaLifecycles::<T>::get(&id)
	}

	/// Returns whether the given ID refers to a valid para.
	///
	/// Paras that are onboarding or offboarding are not included.
	pub fn is_valid_para(id: ParaId) -> bool {
		if let Some(state) = ParaLifecycles::<T>::get(&id) {
			!state.is_onboarding() && !state.is_offboarding()
		} else {
			false
		}
	}

	/// Whether a para ID corresponds to any live parachain.
	///
	/// Includes parachains which will downgrade to a parathread in the future.
	pub fn is_parachain(id: ParaId) -> bool {
		if let Some(state) = ParaLifecycles::<T>::get(&id) {
			state.is_parachain()
		} else {
			false
		}
	}

	/// Whether a para ID corresponds to any live parathread.
	///
	/// Includes parathreads which will upgrade to parachains in the future.
	pub fn is_parathread(id: ParaId) -> bool {
		if let Some(state) = ParaLifecycles::<T>::get(&id) {
			state.is_parathread()
		} else {
			false
		}
	}

	/// If a candidate from the specified parachain were submitted at the current block, this
	/// function returns if that candidate passes the acceptance criteria.
	pub(crate) fn can_upgrade_validation_code(id: ParaId) -> bool {
		FutureCodeHash::<T>::get(&id).is_none() && UpgradeRestrictionSignal::<T>::get(&id).is_none()
	}

	/// Return the session index that should be used for any future scheduled changes.
	fn scheduled_session() -> SessionIndex {
		shared::Pallet::<T>::scheduled_session()
	}

	/// Store the validation code if not already stored, and increase the number of reference.
	///
	/// Returns the weight consumed.
	fn increase_code_ref(code_hash: &ValidationCodeHash, code: &ValidationCode) -> Weight {
		let mut weight = T::DbWeight::get().reads_writes(1, 1);
		<Self as Store>::CodeByHashRefs::mutate(code_hash, |refs| {
			if *refs == 0 {
				weight += T::DbWeight::get().writes(1);
				<Self as Store>::CodeByHash::insert(code_hash, code);
			}
			*refs += 1;
		});
		weight
	}

	/// Decrease the number of reference of the validation code and remove it from storage if zero
	/// is reached.
	///
	/// Returns the weight consumed.
	fn decrease_code_ref(code_hash: &ValidationCodeHash) -> Weight {
		let mut weight = T::DbWeight::get().reads(1);
		let refs = <Self as Store>::CodeByHashRefs::get(code_hash);
		if refs == 0 {
			log::error!(target: LOG_TARGET, "Code refs is already zero for {:?}", code_hash);
			return weight
		}
		if refs <= 1 {
			weight += T::DbWeight::get().writes(2);
			<Self as Store>::CodeByHash::remove(code_hash);
			<Self as Store>::CodeByHashRefs::remove(code_hash);
		} else {
			weight += T::DbWeight::get().writes(1);
			<Self as Store>::CodeByHashRefs::insert(code_hash, refs - 1);
		}
		weight
	}

	/// Test function for triggering a new session in this pallet.
	#[cfg(any(feature = "std", feature = "runtime-benchmarks", test))]
	pub fn test_on_new_session() {
		Self::initializer_on_new_session(&SessionChangeNotification {
			session_index: shared::Pallet::<T>::session_index(),
			..Default::default()
		});
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	pub fn heads_insert(para_id: &ParaId, head_data: HeadData) {
		Heads::<T>::insert(para_id, head_data);
	}

	#[cfg(feature = "runtime-benchmarks")]
	pub(crate) fn initialize_para_now(id: ParaId, genesis: ParaGenesisArgs) -> DispatchResult {
		// first queue this para actions..
		let _ = Self::schedule_para_initialize(id, genesis)?;

		// .. and immediately apply them.
		Self::apply_actions_queue(Self::scheduled_session());

		// ensure it has become a para.
		ensure!(
			ParaLifecycles::<T>::get(id) == Some(ParaLifecycle::Parachain),
			"Parachain not created properly"
		);
		Ok(())
	}
}
