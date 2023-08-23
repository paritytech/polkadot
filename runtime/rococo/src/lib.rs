// Copyright (C) Parity Technologies (UK) Ltd.
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

//! The Rococo runtime for v1 parachains.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit.
#![recursion_limit = "512"]

use pallet_nis::WithMaximumOf;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use primitives::{
	slashing, AccountId, AccountIndex, Balance, BlockNumber, CandidateEvent, CandidateHash,
	CommittedCandidateReceipt, CoreState, DisputeState, ExecutorParams, GroupRotationInfo, Hash,
	Id as ParaId, InboundDownwardMessage, InboundHrmpMessage, Moment, Nonce,
	OccupiedCoreAssumption, PersistedValidationData, ScrapedOnChainVotes, SessionInfo, Signature,
	ValidationCode, ValidationCodeHash, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID,
};
use runtime_common::{
	assigned_slots, auctions, claims, crowdloan, impl_runtime_weights, impls::ToAuthor,
	paras_registrar, paras_sudo_wrapper, prod_or_fast, slots, BlockHashCount, BlockLength,
	SlowAdjustingFeeUpdate,
};
use scale_info::TypeInfo;
use sp_std::{cmp::Ordering, collections::btree_map::BTreeMap, prelude::*};

use runtime_parachains::{
	assigner as parachains_assigner, assigner_on_demand as parachains_assigner_on_demand,
	assigner_parachains as parachains_assigner_parachains,
	configuration as parachains_configuration, disputes as parachains_disputes,
	disputes::slashing as parachains_slashing,
	dmp as parachains_dmp, hrmp as parachains_hrmp, inclusion as parachains_inclusion,
	inclusion::{AggregateMessageOrigin, UmpQueueId},
	initializer as parachains_initializer, origin as parachains_origin, paras as parachains_paras,
	paras_inherent as parachains_paras_inherent,
	runtime_api_impl::v5 as parachains_runtime_api_impl,
	scheduler as parachains_scheduler, session_info as parachains_session_info,
	shared as parachains_shared,
};

use authority_discovery_primitives::AuthorityId as AuthorityDiscoveryId;
use beefy_primitives::{
	ecdsa_crypto::{AuthorityId as BeefyId, Signature as BeefySignature},
	mmr::{BeefyDataProvider, MmrLeafVersion},
};

use frame_support::{
	construct_runtime, parameter_types,
	traits::{
		Contains, EitherOfDiverse, InstanceFilter, KeyOwnerProofSystem, LockIdentifier,
		PrivilegeCmp, ProcessMessage, ProcessMessageError, StorageMapShim, WithdrawReasons,
	},
	weights::{ConstantMultiplier, WeightMeter},
	PalletId,
};
use frame_system::EnsureRoot;
use pallet_grandpa::{fg_primitives, AuthorityId as GrandpaId};
use pallet_im_online::sr25519::AuthorityId as ImOnlineId;
use pallet_session::historical as session_historical;
use pallet_transaction_payment::{CurrencyAdapter, FeeDetails, RuntimeDispatchInfo};
use sp_core::{ConstU128, OpaqueMetadata, H256};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{
		AccountIdLookup, BlakeTwo256, Block as BlockT, ConstU32, ConvertInto,
		Extrinsic as ExtrinsicT, Keccak256, OpaqueKeys, SaturatedConversion, Verify,
	},
	transaction_validity::{TransactionPriority, TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, FixedU128, KeyTypeId, Perbill, Percent, Permill, RuntimeDebug,
};
use sp_staking::SessionIndex;
#[cfg(any(feature = "std", test))]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;
use static_assertions::const_assert;
use xcm::latest::Junction;

pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;

/// Constant values used within the runtime.
use rococo_runtime_constants::{currency::*, fee::*, time::*};

// Weights used in the runtime.
mod weights;

// XCM configurations.
pub mod xcm_config;

mod validator_manager;

impl_runtime_weights!(rococo_runtime_constants);

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/// Runtime version (Rococo).
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("rococo"),
	impl_name: create_runtime_str!("parity-rococo-v2.0"),
	authoring_version: 0,
	spec_version: 9430,
	impl_version: 0,
	#[cfg(not(feature = "disable-runtime-api"))]
	apis: RUNTIME_API_VERSIONS,
	#[cfg(feature = "disable-runtime-api")]
	apis: sp_version::create_apis_vec![[]],
	transaction_version: 22,
	state_version: 1,
};

/// The BABE epoch configuration at genesis.
pub const BABE_GENESIS_EPOCH_CONFIG: babe_primitives::BabeEpochConfiguration =
	babe_primitives::BabeEpochConfiguration {
		c: PRIMARY_PROBABILITY,
		allowed_slots: babe_primitives::AllowedSlots::PrimaryAndSecondaryVRFSlots,
	};

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

/// We currently allow all calls.
pub struct BaseFilter;
impl Contains<RuntimeCall> for BaseFilter {
	fn contains(_call: &RuntimeCall) -> bool {
		true
	}
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub const SS58Prefix: u8 = 42;
}

impl frame_system::Config for Runtime {
	type BaseCallFilter = BaseFilter;
	type BlockWeights = BlockWeights;
	type BlockLength = BlockLength;
	type DbWeight = RocksDbWeight;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type Nonce = Nonce;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = BlockHashCount;
	type Version = Version;
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	type SS58Prefix = SS58Prefix;
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub MaximumSchedulerWeight: Weight = Perbill::from_percent(80) *
		BlockWeights::get().max_block;
	pub const MaxScheduledPerBlock: u32 = 50;
	pub const NoPreimagePostponement: Option<u32> = Some(10);
}

type ScheduleOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 1, 2>,
>;

/// Used the compare the privilege of an origin inside the scheduler.
pub struct OriginPrivilegeCmp;

impl PrivilegeCmp<OriginCaller> for OriginPrivilegeCmp {
	fn cmp_privilege(left: &OriginCaller, right: &OriginCaller) -> Option<Ordering> {
		if left == right {
			return Some(Ordering::Equal)
		}

		match (left, right) {
			// Root is greater than anything.
			(OriginCaller::system(frame_system::RawOrigin::Root), _) => Some(Ordering::Greater),
			// Check which one has more yes votes.
			(
				OriginCaller::Council(pallet_collective::RawOrigin::Members(l_yes_votes, l_count)),
				OriginCaller::Council(pallet_collective::RawOrigin::Members(r_yes_votes, r_count)),
			) => Some((l_yes_votes * r_count).cmp(&(r_yes_votes * l_count))),
			// For every other origin we don't care, as they are not used for `ScheduleOrigin`.
			_ => None,
		}
	}
}

impl pallet_scheduler::Config for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeEvent = RuntimeEvent;
	type PalletsOrigin = OriginCaller;
	type RuntimeCall = RuntimeCall;
	type MaximumWeight = MaximumSchedulerWeight;
	type ScheduleOrigin = ScheduleOrigin;
	type MaxScheduledPerBlock = MaxScheduledPerBlock;
	type WeightInfo = weights::pallet_scheduler::WeightInfo<Runtime>;
	type OriginPrivilegeCmp = OriginPrivilegeCmp;
	type Preimages = Preimage;
}

parameter_types! {
	pub const PreimageBaseDeposit: Balance = deposit(2, 64);
	pub const PreimageByteDeposit: Balance = deposit(0, 1);
}

impl pallet_preimage::Config for Runtime {
	type WeightInfo = weights::pallet_preimage::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type ManagerOrigin = EnsureRoot<AccountId>;
	type BaseDeposit = PreimageBaseDeposit;
	type ByteDeposit = PreimageByteDeposit;
}

parameter_types! {
	pub const ExpectedBlockTime: Moment = MILLISECS_PER_BLOCK;
	pub ReportLongevity: u64 = EpochDurationInBlocks::get() as u64 * 10;
}

impl pallet_babe::Config for Runtime {
	type EpochDuration = EpochDurationInBlocks;
	type ExpectedBlockTime = ExpectedBlockTime;
	// session module is the trigger
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;
	type DisabledValidators = Session;
	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
	type MaxNominators = ConstU32<0>;
	type KeyOwnerProof =
		<Historical as KeyOwnerProofSystem<(KeyTypeId, pallet_babe::AuthorityId)>>::Proof;
	type EquivocationReportSystem =
		pallet_babe::EquivocationReportSystem<Self, Offences, Historical, ReportLongevity>;
}

parameter_types! {
	pub const IndexDeposit: Balance = 100 * CENTS;
}

impl pallet_indices::Config for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_indices::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type Balance = Balance;
	type DustRemoval = ();
	type RuntimeEvent = RuntimeEvent;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type MaxLocks = MaxLocks;
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
	type FreezeIdentifier = ();
	type MaxFreezes = ConstU32<1>;
	type RuntimeHoldReason = RuntimeHoldReason;
	type MaxHolds = ConstU32<1>;
}

parameter_types! {
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
	/// This value increases the priority of `Operational` transactions by adding
	/// a "virtual tip" that's equal to the `OperationalFeeMultiplier * final_fee`.
	pub const OperationalFeeMultiplier: u8 = 5;
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction = CurrencyAdapter<Balances, ToAuthor<Runtime>>;
	type OperationalFeeMultiplier = OperationalFeeMultiplier;
	type WeightToFee = WeightToFee;
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	type FeeMultiplierUpdate = SlowAdjustingFeeUpdate<Self>;
}

parameter_types! {
	pub const MinimumPeriod: u64 = SLOT_DURATION / 2;
}
impl pallet_timestamp::Config for Runtime {
	type Moment = u64;
	type OnTimestampSet = Babe;
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Babe>;
	type EventHandler = ImOnline;
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub im_online: ImOnline,
		pub para_validator: Initializer,
		pub para_assignment: ParaSessionInfo,
		pub authority_discovery: AuthorityDiscovery,
		pub beefy: Beefy,
	}
}

/// Special `ValidatorIdOf` implementation that is just returning the input as result.
pub struct ValidatorIdOf;
impl sp_runtime::traits::Convert<AccountId, Option<AccountId>> for ValidatorIdOf {
	fn convert(a: AccountId) -> Option<AccountId> {
		Some(a)
	}
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = AccountId;
	type ValidatorIdOf = ValidatorIdOf;
	type ShouldEndSession = Babe;
	type NextSessionRotation = Babe;
	type SessionManager = pallet_session::historical::NoteHistoricalRoot<Self, ValidatorManager>;
	type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
}

pub struct FullIdentificationOf;
impl sp_runtime::traits::Convert<AccountId, Option<()>> for FullIdentificationOf {
	fn convert(_: AccountId) -> Option<()> {
		Some(Default::default())
	}
}

impl pallet_session::historical::Config for Runtime {
	type FullIdentification = ();
	type FullIdentificationOf = FullIdentificationOf;
}

parameter_types! {
	pub const SessionsPerEra: SessionIndex = 6;
	pub const BondingDuration: sp_staking::EraIndex = 28;
}

parameter_types! {
	pub LaunchPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1, "ROC_LAUNCH_PERIOD");
	pub VotingPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1 * MINUTES, "ROC_VOTING_PERIOD");
	pub FastTrackVotingPeriod: BlockNumber = prod_or_fast!(3 * HOURS, 1 * MINUTES, "ROC_FAST_TRACK_VOTING_PERIOD");
	pub const MinimumDeposit: Balance = 100 * CENTS;
	pub EnactmentPeriod: BlockNumber = prod_or_fast!(8 * DAYS, 1, "ROC_ENACTMENT_PERIOD");
	pub CooloffPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1 * MINUTES, "ROC_COOLOFF_PERIOD");
	pub const InstantAllowed: bool = true;
	pub const MaxVotes: u32 = 100;
	pub const MaxProposals: u32 = 100;
}

impl pallet_democracy::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type EnactmentPeriod = EnactmentPeriod;
	type VoteLockingPeriod = EnactmentPeriod;
	type LaunchPeriod = LaunchPeriod;
	type VotingPeriod = VotingPeriod;
	type MinimumDeposit = MinimumDeposit;
	type SubmitOrigin = frame_system::EnsureSigned<AccountId>;
	/// A straight majority of the council can decide what their next motion is.
	type ExternalOrigin =
		pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 1, 2>;
	/// A majority can have the next scheduled referendum be a straight majority-carries vote.
	type ExternalMajorityOrigin =
		pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 1, 2>;
	/// A unanimous council can have the next scheduled referendum be a straight default-carries
	/// (NTB) vote.
	type ExternalDefaultOrigin =
		pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 1, 1>;
	/// Two thirds of the technical committee can have an `ExternalMajority/ExternalDefault` vote
	/// be tabled immediately and with a shorter voting/enactment period.
	type FastTrackOrigin =
		pallet_collective::EnsureProportionAtLeast<AccountId, TechnicalCollective, 2, 3>;
	type InstantOrigin =
		pallet_collective::EnsureProportionAtLeast<AccountId, TechnicalCollective, 1, 1>;
	type InstantAllowed = InstantAllowed;
	type FastTrackVotingPeriod = FastTrackVotingPeriod;
	// To cancel a proposal which has been passed, 2/3 of the council must agree to it.
	type CancellationOrigin = EitherOfDiverse<
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 2, 3>,
	>;
	type BlacklistOrigin = EnsureRoot<AccountId>;
	// To cancel a proposal before it has been passed, the technical committee must be unanimous or
	// Root must agree.
	type CancelProposalOrigin = EitherOfDiverse<
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<AccountId, TechnicalCollective, 1, 1>,
	>;
	// Any single technical committee member may veto a coming council proposal, however they can
	// only do it once and it lasts only for the cooloff period.
	type VetoOrigin = pallet_collective::EnsureMember<AccountId, TechnicalCollective>;
	type CooloffPeriod = CooloffPeriod;
	type Slash = Treasury;
	type Scheduler = Scheduler;
	type PalletsOrigin = OriginCaller;
	type MaxVotes = MaxVotes;
	type WeightInfo = weights::pallet_democracy::WeightInfo<Runtime>;
	type MaxProposals = MaxProposals;
	type Preimages = Preimage;
	type MaxDeposits = ConstU32<100>;
	type MaxBlacklisted = ConstU32<100>;
}

parameter_types! {
	pub CouncilMotionDuration: BlockNumber = prod_or_fast!(3 * DAYS, 2 * MINUTES, "ROC_MOTION_DURATION");
	pub const CouncilMaxProposals: u32 = 100;
	pub const CouncilMaxMembers: u32 = 100;
	pub MaxProposalWeight: Weight = Perbill::from_percent(50) * BlockWeights::get().max_block;
}

type CouncilCollective = pallet_collective::Instance1;
impl pallet_collective::Config<CouncilCollective> for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type Proposal = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type MotionDuration = CouncilMotionDuration;
	type MaxProposals = CouncilMaxProposals;
	type MaxMembers = CouncilMaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type SetMembersOrigin = EnsureRoot<AccountId>;
	type WeightInfo = weights::pallet_collective_council::WeightInfo<Runtime>;
	type MaxProposalWeight = MaxProposalWeight;
}

parameter_types! {
	pub const CandidacyBond: Balance = 100 * CENTS;
	// 1 storage item created, key size is 32 bytes, value size is 16+16.
	pub const VotingBondBase: Balance = deposit(1, 64);
	// additional data per vote is 32 bytes (account id).
	pub const VotingBondFactor: Balance = deposit(0, 32);
	/// Daily council elections
	pub TermDuration: BlockNumber = prod_or_fast!(24 * HOURS, 2 * MINUTES, "ROC_TERM_DURATION");
	pub const DesiredMembers: u32 = 19;
	pub const DesiredRunnersUp: u32 = 19;
	pub const MaxVoters: u32 = 10 * 1000;
	pub const MaxVotesPerVoter: u32 = 16;
	pub const MaxCandidates: u32 = 1000;
	pub const PhragmenElectionPalletId: LockIdentifier = *b"phrelect";
}

// Make sure that there are no more than MaxMembers members elected via phragmen.
const_assert!(DesiredMembers::get() <= CouncilMaxMembers::get());

impl pallet_elections_phragmen::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type ChangeMembers = Council;
	type InitializeMembers = Council;
	type CurrencyToVote = runtime_common::CurrencyToVote;
	type CandidacyBond = CandidacyBond;
	type VotingBondBase = VotingBondBase;
	type VotingBondFactor = VotingBondFactor;
	type LoserCandidate = Treasury;
	type KickedMember = Treasury;
	type DesiredMembers = DesiredMembers;
	type DesiredRunnersUp = DesiredRunnersUp;
	type TermDuration = TermDuration;
	type MaxVoters = MaxVoters;
	type MaxVotesPerVoter = MaxVotesPerVoter;
	type MaxCandidates = MaxCandidates;
	type PalletId = PhragmenElectionPalletId;
	type WeightInfo = weights::pallet_elections_phragmen::WeightInfo<Runtime>;
}

parameter_types! {
	pub TechnicalMotionDuration: BlockNumber = prod_or_fast!(3 * DAYS, 2 * MINUTES, "ROC_MOTION_DURATION");
	pub const TechnicalMaxProposals: u32 = 100;
	pub const TechnicalMaxMembers: u32 = 100;
}

type TechnicalCollective = pallet_collective::Instance2;
impl pallet_collective::Config<TechnicalCollective> for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type Proposal = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type MotionDuration = TechnicalMotionDuration;
	type MaxProposals = TechnicalMaxProposals;
	type MaxMembers = TechnicalMaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type SetMembersOrigin = EnsureRoot<AccountId>;
	type WeightInfo = weights::pallet_collective_technical_committee::WeightInfo<Runtime>;
	type MaxProposalWeight = MaxProposalWeight;
}

type MoreThanHalfCouncil = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionMoreThan<AccountId, CouncilCollective, 1, 2>,
>;

impl pallet_membership::Config<pallet_membership::Instance1> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AddOrigin = MoreThanHalfCouncil;
	type RemoveOrigin = MoreThanHalfCouncil;
	type SwapOrigin = MoreThanHalfCouncil;
	type ResetOrigin = MoreThanHalfCouncil;
	type PrimeOrigin = MoreThanHalfCouncil;
	type MembershipInitialized = TechnicalCommittee;
	type MembershipChanged = TechnicalCommittee;
	type MaxMembers = TechnicalMaxMembers;
	type WeightInfo = weights::pallet_membership::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ProposalBond: Permill = Permill::from_percent(5);
	pub const ProposalBondMinimum: Balance = 2000 * CENTS;
	pub const ProposalBondMaximum: Balance = 1 * GRAND;
	pub const SpendPeriod: BlockNumber = 6 * DAYS;
	pub const Burn: Permill = Permill::from_perthousand(2);
	pub const TreasuryPalletId: PalletId = PalletId(*b"py/trsry");

	pub const TipCountdown: BlockNumber = 1 * DAYS;
	pub const TipFindersFee: Percent = Percent::from_percent(20);
	pub const TipReportDepositBase: Balance = 100 * CENTS;
	pub const DataDepositPerByte: Balance = 1 * CENTS;
	pub const MaxApprovals: u32 = 100;
	pub const MaxAuthorities: u32 = 100_000;
	pub const MaxKeys: u32 = 10_000;
	pub const MaxPeerInHeartbeats: u32 = 10_000;
}

type ApproveOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 3, 5>,
>;

impl pallet_treasury::Config for Runtime {
	type PalletId = TreasuryPalletId;
	type Currency = Balances;
	type ApproveOrigin = ApproveOrigin;
	type RejectOrigin = MoreThanHalfCouncil;
	type RuntimeEvent = RuntimeEvent;
	type OnSlash = Treasury;
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type ProposalBondMaximum = ProposalBondMaximum;
	type SpendPeriod = SpendPeriod;
	type Burn = Burn;
	type BurnDestination = Society;
	type MaxApprovals = MaxApprovals;
	type WeightInfo = weights::pallet_treasury::WeightInfo<Runtime>;
	type SpendFunds = Bounties;
	type SpendOrigin = frame_support::traits::NeverEnsureOrigin<Balance>;
}

parameter_types! {
	pub const BountyDepositBase: Balance = 100 * CENTS;
	pub const BountyDepositPayoutDelay: BlockNumber = 4 * DAYS;
	pub const BountyUpdatePeriod: BlockNumber = 90 * DAYS;
	pub const MaximumReasonLength: u32 = 16384;
	pub const CuratorDepositMultiplier: Permill = Permill::from_percent(50);
	pub const CuratorDepositMin: Balance = 10 * CENTS;
	pub const CuratorDepositMax: Balance = 500 * CENTS;
	pub const BountyValueMinimum: Balance = 200 * CENTS;
}

impl pallet_bounties::Config for Runtime {
	type BountyDepositBase = BountyDepositBase;
	type BountyDepositPayoutDelay = BountyDepositPayoutDelay;
	type BountyUpdatePeriod = BountyUpdatePeriod;
	type CuratorDepositMultiplier = CuratorDepositMultiplier;
	type CuratorDepositMin = CuratorDepositMin;
	type CuratorDepositMax = CuratorDepositMax;
	type BountyValueMinimum = BountyValueMinimum;
	type ChildBountyManager = ChildBounties;
	type DataDepositPerByte = DataDepositPerByte;
	type RuntimeEvent = RuntimeEvent;
	type MaximumReasonLength = MaximumReasonLength;
	type WeightInfo = weights::pallet_bounties::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxActiveChildBountyCount: u32 = 100;
	pub const ChildBountyValueMinimum: Balance = BountyValueMinimum::get() / 10;
}

impl pallet_child_bounties::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type MaxActiveChildBountyCount = MaxActiveChildBountyCount;
	type ChildBountyValueMinimum = ChildBountyValueMinimum;
	type WeightInfo = weights::pallet_child_bounties::WeightInfo<Runtime>;
}

impl pallet_tips::Config for Runtime {
	type MaximumReasonLength = MaximumReasonLength;
	type DataDepositPerByte = DataDepositPerByte;
	type Tippers = PhragmenElection;
	type TipCountdown = TipCountdown;
	type TipFindersFee = TipFindersFee;
	type TipReportDepositBase = TipReportDepositBase;
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_tips::WeightInfo<Runtime>;
}

impl pallet_offences::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type IdentificationTuple = pallet_session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = ();
}

impl pallet_authority_discovery::Config for Runtime {
	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const ImOnlineUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl pallet_im_online::Config for Runtime {
	type AuthorityId = ImOnlineId;
	type RuntimeEvent = RuntimeEvent;
	type ValidatorSet = Historical;
	type NextSessionRotation = Babe;
	type ReportUnresponsiveness = Offences;
	type UnsignedPriority = ImOnlineUnsignedPriority;
	type WeightInfo = weights::pallet_im_online::WeightInfo<Runtime>;
	type MaxKeys = MaxKeys;
	type MaxPeerInHeartbeats = MaxPeerInHeartbeats;
}

parameter_types! {
	pub const MaxSetIdSessionEntries: u32 = BondingDuration::get() * SessionsPerEra::get();
}

impl pallet_grandpa::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
	type MaxNominators = ConstU32<0>;
	type MaxSetIdSessionEntries = MaxSetIdSessionEntries;
	type KeyOwnerProof = <Historical as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;
	type EquivocationReportSystem =
		pallet_grandpa::EquivocationReportSystem<Self, Offences, Historical, ReportLongevity>;
}

/// Submits a transaction with the node's public and signature type. Adheres to the signed extension
/// format of the chain.
impl<LocalCall> frame_system::offchain::CreateSignedTransaction<LocalCall> for Runtime
where
	RuntimeCall: From<LocalCall>,
{
	fn create_transaction<C: frame_system::offchain::AppCrypto<Self::Public, Self::Signature>>(
		call: RuntimeCall,
		public: <Signature as Verify>::Signer,
		account: AccountId,
		nonce: <Runtime as frame_system::Config>::Nonce,
	) -> Option<(RuntimeCall, <UncheckedExtrinsic as ExtrinsicT>::SignaturePayload)> {
		use sp_runtime::traits::StaticLookup;
		// take the biggest period possible.
		let period =
			BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;

		let current_block = System::block_number()
			.saturated_into::<u64>()
			// The `System::block_number` is initialized with `n+1`,
			// so the actual block number is `n`.
			.saturating_sub(1);
		let tip = 0;
		let extra: SignedExtra = (
			frame_system::CheckNonZeroSender::<Runtime>::new(),
			frame_system::CheckSpecVersion::<Runtime>::new(),
			frame_system::CheckTxVersion::<Runtime>::new(),
			frame_system::CheckGenesis::<Runtime>::new(),
			frame_system::CheckMortality::<Runtime>::from(generic::Era::mortal(
				period,
				current_block,
			)),
			frame_system::CheckNonce::<Runtime>::from(nonce),
			frame_system::CheckWeight::<Runtime>::new(),
			pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
		);
		let raw_payload = SignedPayload::new(call, extra)
			.map_err(|e| {
				log::warn!("Unable to create signed payload: {:?}", e);
			})
			.ok()?;
		let signature = raw_payload.using_encoded(|payload| C::sign(payload, public))?;
		let (call, extra, _) = raw_payload.deconstruct();
		let address = <Runtime as frame_system::Config>::Lookup::unlookup(account);
		Some((call, (address, signature, extra)))
	}
}

impl frame_system::offchain::SigningTypes for Runtime {
	type Public = <Signature as Verify>::Signer;
	type Signature = Signature;
}

impl<C> frame_system::offchain::SendTransactionTypes<C> for Runtime
where
	RuntimeCall: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = RuntimeCall;
}

parameter_types! {
	pub Prefix: &'static [u8] = b"Pay ROCs to the Rococo account:";
}

impl claims::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type VestingSchedule = Vesting;
	type Prefix = Prefix;
	type MoveClaimOrigin =
		pallet_collective::EnsureProportionMoreThan<AccountId, CouncilCollective, 1, 2>;
	type WeightInfo = weights::runtime_common_claims::WeightInfo<Runtime>;
}

parameter_types! {
	// Minimum 100 bytes/ROC deposited (1 CENT/byte)
	pub const BasicDeposit: Balance = 1000 * CENTS;       // 258 bytes on-chain
	pub const FieldDeposit: Balance = 250 * CENTS;        // 66 bytes on-chain
	pub const SubAccountDeposit: Balance = 200 * CENTS;   // 53 bytes on-chain
	pub const MaxSubAccounts: u32 = 100;
	pub const MaxAdditionalFields: u32 = 100;
	pub const MaxRegistrars: u32 = 20;
}

impl pallet_identity::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type BasicDeposit = BasicDeposit;
	type FieldDeposit = FieldDeposit;
	type SubAccountDeposit = SubAccountDeposit;
	type MaxSubAccounts = MaxSubAccounts;
	type MaxAdditionalFields = MaxAdditionalFields;
	type MaxRegistrars = MaxRegistrars;
	type Slashed = Treasury;
	type ForceOrigin = MoreThanHalfCouncil;
	type RegistrarOrigin = MoreThanHalfCouncil;
	type WeightInfo = weights::pallet_identity::WeightInfo<Runtime>;
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = deposit(0, 32);
	pub const MaxSignatories: u32 = 100;
}

impl pallet_multisig::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = MaxSignatories;
	type WeightInfo = weights::pallet_multisig::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ConfigDepositBase: Balance = 500 * CENTS;
	pub const FriendDepositFactor: Balance = 50 * CENTS;
	pub const MaxFriends: u16 = 9;
	pub const RecoveryDeposit: Balance = 500 * CENTS;
}

impl pallet_recovery::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type ConfigDepositBase = ConfigDepositBase;
	type FriendDepositFactor = FriendDepositFactor;
	type MaxFriends = MaxFriends;
	type RecoveryDeposit = RecoveryDeposit;
}

parameter_types! {
	pub const SocietyPalletId: PalletId = PalletId(*b"py/socie");
}

impl pallet_society::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type GraceStrikes = ConstU32<1>;
	type PeriodSpend = ConstU128<{ 50_000 * CENTS }>;
	type VotingPeriod = ConstU32<{ 5 * DAYS }>;
	type ClaimPeriod = ConstU32<{ 2 * DAYS }>;
	type MaxLockDuration = ConstU32<{ 36 * 30 * DAYS }>;
	type FounderSetOrigin = EnsureRoot<AccountId>;
	type ChallengePeriod = ConstU32<{ 7 * DAYS }>;
	type MaxPayouts = ConstU32<8>;
	type MaxBids = ConstU32<512>;
	type PalletId = SocietyPalletId;
	type WeightInfo = ();
}

parameter_types! {
	pub const MinVestedTransfer: Balance = 100 * CENTS;
	pub UnvestedFundsAllowedWithdrawReasons: WithdrawReasons =
		WithdrawReasons::except(WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE);
}

impl pallet_vesting::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type BlockNumberToBalance = ConvertInto;
	type MinVestedTransfer = MinVestedTransfer;
	type WeightInfo = weights::pallet_vesting::WeightInfo<Runtime>;
	type UnvestedFundsAllowedWithdrawReasons = UnvestedFundsAllowedWithdrawReasons;
	const MAX_VESTING_SCHEDULES: u32 = 28;
}

parameter_types! {
	// One storage item; key size 32, value size 8; .
	pub const ProxyDepositBase: Balance = deposit(1, 8);
	// Additional storage item size of 33 bytes.
	pub const ProxyDepositFactor: Balance = deposit(0, 33);
	pub const MaxProxies: u16 = 32;
	pub const AnnouncementDepositBase: Balance = deposit(1, 8);
	pub const AnnouncementDepositFactor: Balance = deposit(0, 66);
	pub const MaxPending: u16 = 32;
}

/// The type used to represent the kinds of proxying allowed.
#[derive(
	Copy,
	Clone,
	Eq,
	PartialEq,
	Ord,
	PartialOrd,
	Encode,
	Decode,
	RuntimeDebug,
	MaxEncodedLen,
	TypeInfo,
)]
pub enum ProxyType {
	Any,
	NonTransfer,
	Governance,
	IdentityJudgement,
	CancelProxy,
	Auction,
	Society,
	OnDemandOrdering,
}
impl Default for ProxyType {
	fn default() -> Self {
		Self::Any
	}
}
impl InstanceFilter<RuntimeCall> for ProxyType {
	fn filter(&self, c: &RuntimeCall) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::NonTransfer => matches!(
				c,
				RuntimeCall::System(..) |
				RuntimeCall::Babe(..) |
				RuntimeCall::Timestamp(..) |
				RuntimeCall::Indices(pallet_indices::Call::claim {..}) |
				RuntimeCall::Indices(pallet_indices::Call::free {..}) |
				RuntimeCall::Indices(pallet_indices::Call::freeze {..}) |
				// Specifically omitting Indices `transfer`, `force_transfer`
				// Specifically omitting the entire Balances pallet
				RuntimeCall::Session(..) |
				RuntimeCall::Grandpa(..) |
				RuntimeCall::ImOnline(..) |
				RuntimeCall::Democracy(..) |
				RuntimeCall::Council(..) |
				RuntimeCall::TechnicalCommittee(..) |
				RuntimeCall::PhragmenElection(..) |
				RuntimeCall::TechnicalMembership(..) |
				RuntimeCall::Treasury(..) |
				RuntimeCall::Bounties(..) |
				RuntimeCall::ChildBounties(..) |
				RuntimeCall::Tips(..) |
				RuntimeCall::Claims(..) |
				RuntimeCall::Utility(..) |
				RuntimeCall::Identity(..) |
				RuntimeCall::Society(..) |
				RuntimeCall::Recovery(pallet_recovery::Call::as_recovered {..}) |
				RuntimeCall::Recovery(pallet_recovery::Call::vouch_recovery {..}) |
				RuntimeCall::Recovery(pallet_recovery::Call::claim_recovery {..}) |
				RuntimeCall::Recovery(pallet_recovery::Call::close_recovery {..}) |
				RuntimeCall::Recovery(pallet_recovery::Call::remove_recovery {..}) |
				RuntimeCall::Recovery(pallet_recovery::Call::cancel_recovered {..}) |
				// Specifically omitting Recovery `create_recovery`, `initiate_recovery`
				RuntimeCall::Vesting(pallet_vesting::Call::vest {..}) |
				RuntimeCall::Vesting(pallet_vesting::Call::vest_other {..}) |
				// Specifically omitting Vesting `vested_transfer`, and `force_vested_transfer`
				RuntimeCall::Scheduler(..) |
				RuntimeCall::Proxy(..) |
				RuntimeCall::Multisig(..) |
				RuntimeCall::Nis(..) |
				RuntimeCall::Registrar(paras_registrar::Call::register {..}) |
				RuntimeCall::Registrar(paras_registrar::Call::deregister {..}) |
				// Specifically omitting Registrar `swap`
				RuntimeCall::Registrar(paras_registrar::Call::reserve {..}) |
				RuntimeCall::Crowdloan(..) |
				RuntimeCall::Slots(..) |
				RuntimeCall::Auctions(..) // Specifically omitting the entire XCM Pallet
			),
			ProxyType::Governance =>
				matches!(
					c,
					RuntimeCall::Democracy(..) |
						RuntimeCall::Council(..) | RuntimeCall::TechnicalCommittee(..) |
						RuntimeCall::PhragmenElection(..) |
						RuntimeCall::Treasury(..) |
						RuntimeCall::Bounties(..) |
						RuntimeCall::Tips(..) | RuntimeCall::Utility(..) |
						RuntimeCall::ChildBounties(..)
				),
			ProxyType::IdentityJudgement => matches!(
				c,
				RuntimeCall::Identity(pallet_identity::Call::provide_judgement { .. }) |
					RuntimeCall::Utility(..)
			),
			ProxyType::CancelProxy => {
				matches!(c, RuntimeCall::Proxy(pallet_proxy::Call::reject_announcement { .. }))
			},
			ProxyType::Auction => matches!(
				c,
				RuntimeCall::Auctions { .. } |
					RuntimeCall::Crowdloan { .. } |
					RuntimeCall::Registrar { .. } |
					RuntimeCall::Multisig(..) |
					RuntimeCall::Slots { .. }
			),
			ProxyType::Society => matches!(c, RuntimeCall::Society(..)),
			ProxyType::OnDemandOrdering => matches!(c, RuntimeCall::OnDemandAssignmentProvider(..)),
		}
	}
	fn is_superset(&self, o: &Self) -> bool {
		match (self, o) {
			(x, y) if x == y => true,
			(ProxyType::Any, _) => true,
			(_, ProxyType::Any) => false,
			(ProxyType::NonTransfer, _) => true,
			_ => false,
		}
	}
}

impl pallet_proxy::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type ProxyType = ProxyType;
	type ProxyDepositBase = ProxyDepositBase;
	type ProxyDepositFactor = ProxyDepositFactor;
	type MaxProxies = MaxProxies;
	type WeightInfo = weights::pallet_proxy::WeightInfo<Runtime>;
	type MaxPending = MaxPending;
	type CallHasher = BlakeTwo256;
	type AnnouncementDepositBase = AnnouncementDepositBase;
	type AnnouncementDepositFactor = AnnouncementDepositFactor;
}

impl parachains_origin::Config for Runtime {}

impl parachains_configuration::Config for Runtime {
	type WeightInfo = weights::runtime_parachains_configuration::WeightInfo<Runtime>;
}

impl parachains_shared::Config for Runtime {}

impl parachains_session_info::Config for Runtime {
	type ValidatorSet = Historical;
}

/// Special `RewardValidators` that does nothing ;)
pub struct RewardValidators;
impl runtime_parachains::inclusion::RewardValidators for RewardValidators {
	fn reward_backing(_: impl IntoIterator<Item = ValidatorIndex>) {}
	fn reward_bitfields(_: impl IntoIterator<Item = ValidatorIndex>) {}
}

impl parachains_inclusion::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type DisputesHandler = ParasDisputes;
	type RewardValidators = RewardValidators;
	type MessageQueue = MessageQueue;
	type WeightInfo = weights::runtime_parachains_inclusion::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl parachains_paras::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::runtime_parachains_paras::WeightInfo<Runtime>;
	type UnsignedPriority = ParasUnsignedPriority;
	type QueueFootprinter = ParaInclusion;
	type NextSessionRotation = Babe;
}

parameter_types! {
	/// Amount of weight that can be spent per block to service messages.
	///
	/// # WARNING
	///
	/// This is not a good value for para-chains since the `Scheduler` already uses up to 80% block weight.
	pub MessageQueueServiceWeight: Weight = Perbill::from_percent(20) * BlockWeights::get().max_block;
	pub const MessageQueueHeapSize: u32 = 32 * 1024;
	pub const MessageQueueMaxStale: u32 = 96;
}

/// Message processor to handle any messages that were enqueued into the `MessageQueue` pallet.
pub struct MessageProcessor;
impl ProcessMessage for MessageProcessor {
	type Origin = AggregateMessageOrigin;

	fn process_message(
		message: &[u8],
		origin: Self::Origin,
		meter: &mut WeightMeter,
		id: &mut [u8; 32],
	) -> Result<bool, ProcessMessageError> {
		let para = match origin {
			AggregateMessageOrigin::Ump(UmpQueueId::Para(para)) => para,
		};
		xcm_builder::ProcessXcmMessage::<
			Junction,
			xcm_executor::XcmExecutor<xcm_config::XcmConfig>,
			RuntimeCall,
		>::process_message(message, Junction::Parachain(para.into()), meter, id)
	}
}

impl pallet_message_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Size = u32;
	type HeapSize = MessageQueueHeapSize;
	type MaxStale = MessageQueueMaxStale;
	type ServiceWeight = MessageQueueServiceWeight;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type MessageProcessor = MessageProcessor;
	#[cfg(feature = "runtime-benchmarks")]
	type MessageProcessor =
		pallet_message_queue::mock_helpers::NoopMessageProcessor<AggregateMessageOrigin>;
	type QueueChangeHandler = ParaInclusion;
	type QueuePausedQuery = ();
	type WeightInfo = weights::pallet_message_queue::WeightInfo<Runtime>;
}

impl parachains_dmp::Config for Runtime {}

impl parachains_hrmp::Config for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeEvent = RuntimeEvent;
	type ChannelManager = EnsureRoot<AccountId>;
	type Currency = Balances;
	type WeightInfo = weights::runtime_parachains_hrmp::WeightInfo<Runtime>;
}

impl parachains_paras_inherent::Config for Runtime {
	type WeightInfo = weights::runtime_parachains_paras_inherent::WeightInfo<Runtime>;
}

impl parachains_scheduler::Config for Runtime {
	type AssignmentProvider = ParaAssignmentProvider;
}

parameter_types! {
	pub const OnDemandTrafficDefaultValue: FixedU128 = FixedU128::from_u32(1);
}

impl parachains_assigner_on_demand::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type TrafficDefaultValue = OnDemandTrafficDefaultValue;
	type WeightInfo = weights::runtime_parachains_assigner_on_demand::WeightInfo<Runtime>;
}

impl parachains_assigner_parachains::Config for Runtime {}

impl parachains_assigner::Config for Runtime {
	type OnDemandAssignmentProvider = OnDemandAssignmentProvider;
	type ParachainsAssignmentProvider = ParachainsAssignmentProvider;
}

impl parachains_initializer::Config for Runtime {
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type ForceOrigin = EnsureRoot<AccountId>;
	type WeightInfo = weights::runtime_parachains_initializer::WeightInfo<Runtime>;
}

impl parachains_disputes::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RewardValidators = ();
	type SlashingHandler = parachains_slashing::SlashValidatorsForDisputes<ParasSlashing>;
	type WeightInfo = weights::runtime_parachains_disputes::WeightInfo<Runtime>;
}

impl parachains_slashing::Config for Runtime {
	type KeyOwnerProofSystem = Historical;
	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, ValidatorId)>>::Proof;
	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		ValidatorId,
	)>>::IdentificationTuple;
	type HandleReports = parachains_slashing::SlashingReportHandler<
		Self::KeyOwnerIdentification,
		Offences,
		ReportLongevity,
	>;
	type WeightInfo = parachains_slashing::TestWeightInfo;
	type BenchmarkingConfig = parachains_slashing::BenchConfig<200>;
}

parameter_types! {
	pub const ParaDeposit: Balance = 40 * UNITS;
}

impl paras_registrar::Config for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type OnSwap = (Crowdloan, Slots);
	type ParaDeposit = ParaDeposit;
	type DataDepositPerByte = DataDepositPerByte;
	type WeightInfo = weights::runtime_common_paras_registrar::WeightInfo<Runtime>;
}

parameter_types! {
	pub LeasePeriod: BlockNumber = prod_or_fast!(1 * DAYS, 1 * DAYS, "ROC_LEASE_PERIOD");
}

impl slots::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type Registrar = Registrar;
	type LeasePeriod = LeasePeriod;
	type LeaseOffset = ();
	type ForceOrigin = MoreThanHalfCouncil;
	type WeightInfo = weights::runtime_common_slots::WeightInfo<Runtime>;
}

parameter_types! {
	pub const CrowdloanId: PalletId = PalletId(*b"py/cfund");
	pub const SubmissionDeposit: Balance = 3 * GRAND;
	pub const MinContribution: Balance = 3_000 * CENTS;
	pub const RemoveKeysLimit: u32 = 1000;
	// Allow 32 bytes for an additional memo to a crowdloan.
	pub const MaxMemoLength: u8 = 32;
}

impl crowdloan::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type PalletId = CrowdloanId;
	type SubmissionDeposit = SubmissionDeposit;
	type MinContribution = MinContribution;
	type RemoveKeysLimit = RemoveKeysLimit;
	type Registrar = Registrar;
	type Auctioneer = Auctions;
	type MaxMemoLength = MaxMemoLength;
	type WeightInfo = weights::runtime_common_crowdloan::WeightInfo<Runtime>;
}

parameter_types! {
	// The average auction is 7 days long, so this will be 70% for ending period.
	// 5 Days = 72000 Blocks @ 6 sec per block
	pub const EndingPeriod: BlockNumber = 5 * DAYS;
	// ~ 1000 samples per day -> ~ 20 blocks per sample -> 2 minute samples
	pub const SampleLength: BlockNumber = 2 * MINUTES;
}

type AuctionInitiate = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 2, 3>,
>;

impl auctions::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Leaser = Slots;
	type Registrar = Registrar;
	type EndingPeriod = EndingPeriod;
	type SampleLength = SampleLength;
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type InitiateOrigin = AuctionInitiate;
	type WeightInfo = weights::runtime_common_auctions::WeightInfo<Runtime>;
}

type NisCounterpartInstance = pallet_balances::Instance2;
impl pallet_balances::Config<NisCounterpartInstance> for Runtime {
	type Balance = Balance;
	type DustRemoval = ();
	type RuntimeEvent = RuntimeEvent;
	type ExistentialDeposit = ConstU128<10_000_000_000>; // One RTC cent
	type AccountStore = StorageMapShim<
		pallet_balances::Account<Runtime, NisCounterpartInstance>,
		AccountId,
		pallet_balances::AccountData<u128>,
	>;
	type MaxLocks = ConstU32<4>;
	type MaxReserves = ConstU32<4>;
	type ReserveIdentifier = [u8; 8];
	type WeightInfo = weights::pallet_balances_nis_counterpart_balances::WeightInfo<Runtime>;
	type RuntimeHoldReason = RuntimeHoldReason;
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	pub const NisBasePeriod: BlockNumber = 30 * DAYS;
	pub const MinBid: Balance = 100 * UNITS;
	pub MinReceipt: Perquintill = Perquintill::from_rational(1u64, 10_000_000u64);
	pub const IntakePeriod: BlockNumber = 5 * MINUTES;
	pub MaxIntakeWeight: Weight = MAXIMUM_BLOCK_WEIGHT / 10;
	pub const ThawThrottle: (Perquintill, BlockNumber) = (Perquintill::from_percent(25), 5);
	pub storage NisTarget: Perquintill = Perquintill::zero();
	pub const NisPalletId: PalletId = PalletId(*b"py/nis  ");
}

impl pallet_nis::Config for Runtime {
	type WeightInfo = weights::pallet_nis::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type CurrencyBalance = Balance;
	type FundOrigin = frame_system::EnsureSigned<AccountId>;
	type Counterpart = NisCounterpartBalances;
	type CounterpartAmount = WithMaximumOf<ConstU128<21_000_000_000_000_000_000u128>>;
	type Deficit = (); // Mint
	type IgnoredIssuance = ();
	type Target = NisTarget;
	type PalletId = NisPalletId;
	type QueueCount = ConstU32<300>;
	type MaxQueueLen = ConstU32<1000>;
	type FifoQueueLen = ConstU32<250>;
	type BasePeriod = NisBasePeriod;
	type MinBid = MinBid;
	type MinReceipt = MinReceipt;
	type IntakePeriod = IntakePeriod;
	type MaxIntakeWeight = MaxIntakeWeight;
	type ThawThrottle = ThawThrottle;
	type RuntimeHoldReason = RuntimeHoldReason;
}

parameter_types! {
	pub const BeefySetIdSessionEntries: u32 = BondingDuration::get() * SessionsPerEra::get();
}

impl pallet_beefy::Config for Runtime {
	type BeefyId = BeefyId;
	type MaxAuthorities = MaxAuthorities;
	type MaxNominators = ConstU32<0>;
	type MaxSetIdSessionEntries = BeefySetIdSessionEntries;
	type OnNewValidatorSet = MmrLeaf;
	type WeightInfo = ();
	type KeyOwnerProof = <Historical as KeyOwnerProofSystem<(KeyTypeId, BeefyId)>>::Proof;
	type EquivocationReportSystem =
		pallet_beefy::EquivocationReportSystem<Self, Offences, Historical, ReportLongevity>;
}

/// MMR helper types.
mod mmr {
	use super::Runtime;
	pub use pallet_mmr::primitives::*;

	pub type Leaf = <<Runtime as pallet_mmr::Config>::LeafData as LeafDataProvider>::LeafData;
	pub type Hashing = <Runtime as pallet_mmr::Config>::Hashing;
	pub type Hash = <Hashing as sp_runtime::traits::Hash>::Output;
}

impl pallet_mmr::Config for Runtime {
	const INDEXING_PREFIX: &'static [u8] = mmr::INDEXING_PREFIX;
	type Hashing = Keccak256;
	type OnNewRoot = pallet_beefy_mmr::DepositBeefyDigest<Runtime>;
	type WeightInfo = ();
	type LeafData = pallet_beefy_mmr::Pallet<Runtime>;
}

parameter_types! {
	/// Version of the produced MMR leaf.
	///
	/// The version consists of two parts;
	/// - `major` (3 bits)
	/// - `minor` (5 bits)
	///
	/// `major` should be updated only if decoding the previous MMR Leaf format from the payload
	/// is not possible (i.e. backward incompatible change).
	/// `minor` should be updated if fields are added to the previous MMR Leaf, which given SCALE
	/// encoding does not prevent old leafs from being decoded.
	///
	/// Hence we expect `major` to be changed really rarely (think never).
	/// See [`MmrLeafVersion`] type documentation for more details.
	pub LeafVersion: MmrLeafVersion = MmrLeafVersion::new(0, 0);
}

pub struct ParaHeadsRootProvider;
impl BeefyDataProvider<H256> for ParaHeadsRootProvider {
	fn extra_data() -> H256 {
		let mut para_heads: Vec<(u32, Vec<u8>)> = Paras::parachains()
			.into_iter()
			.filter_map(|id| Paras::para_head(&id).map(|head| (id.into(), head.0)))
			.collect();
		para_heads.sort();
		binary_merkle_tree::merkle_root::<mmr::Hashing, _>(
			para_heads.into_iter().map(|pair| pair.encode()),
		)
		.into()
	}
}

impl pallet_beefy_mmr::Config for Runtime {
	type LeafVersion = LeafVersion;
	type BeefyAuthorityToMerkleLeaf = pallet_beefy_mmr::BeefyEcdsaToEthereum;
	type LeafExtra = H256;
	type BeefyDataProvider = ParaHeadsRootProvider;
}

impl paras_sudo_wrapper::Config for Runtime {}

parameter_types! {
	pub const PermanentSlotLeasePeriodLength: u32 = 365;
	pub const TemporarySlotLeasePeriodLength: u32 = 5;
	pub const MaxTemporarySlotPerLeasePeriod: u32 = 5;
}

impl assigned_slots::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AssignSlotOrigin = EnsureRoot<AccountId>;
	type Leaser = Slots;
	type PermanentSlotLeasePeriodLength = PermanentSlotLeasePeriodLength;
	type TemporarySlotLeasePeriodLength = TemporarySlotLeasePeriodLength;
	type MaxTemporarySlotPerLeasePeriod = MaxTemporarySlotPerLeasePeriod;
	type WeightInfo = weights::runtime_common_assigned_slots::WeightInfo<Runtime>;
}

impl validator_manager::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type PrivilegedOrigin = EnsureRoot<AccountId>;
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type WeightInfo = weights::pallet_sudo::WeightInfo<Runtime>;
}

construct_runtime! {
	pub enum Runtime
	{
		// Basic stuff; balances is uncallable initially.
		System: frame_system::{Pallet, Call, Storage, Config<T>, Event<T>} = 0,

		// Babe must be before session.
		Babe: pallet_babe::{Pallet, Call, Storage, Config<T>, ValidateUnsigned} = 1,

		Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent} = 2,
		Indices: pallet_indices::{Pallet, Call, Storage, Config<T>, Event<T>} = 3,
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>} = 4,
		TransactionPayment: pallet_transaction_payment::{Pallet, Storage, Event<T>} = 33,

		// Consensus support.
		// Authorship must be before session in order to note author in the correct session and era
		// for im-online.
		Authorship: pallet_authorship::{Pallet, Storage} = 5,
		Offences: pallet_offences::{Pallet, Storage, Event} = 7,
		Historical: session_historical::{Pallet} = 34,

		// BEEFY Bridges support.
		Beefy: pallet_beefy::{Pallet, Call, Storage, Config<T>, ValidateUnsigned} = 240,
		// MMR leaf construction must be before session in order to have leaf contents
		// refer to block<N-1> consistently. see substrate issue #11797 for details.
		Mmr: pallet_mmr::{Pallet, Storage} = 241,
		MmrLeaf: pallet_beefy_mmr::{Pallet, Storage} = 242,

		Session: pallet_session::{Pallet, Call, Storage, Event, Config<T>} = 8,
		Grandpa: pallet_grandpa::{Pallet, Call, Storage, Config<T>, Event, ValidateUnsigned} = 10,
		ImOnline: pallet_im_online::{Pallet, Call, Storage, Event<T>, ValidateUnsigned, Config<T>} = 11,
		AuthorityDiscovery: pallet_authority_discovery::{Pallet, Config<T>} = 12,

		// Governance stuff; uncallable initially.
		Democracy: pallet_democracy::{Pallet, Call, Storage, Config<T>, Event<T>} = 13,
		Council: pallet_collective::<Instance1>::{Pallet, Call, Storage, Origin<T>, Event<T>, Config<T>} = 14,
		TechnicalCommittee: pallet_collective::<Instance2>::{Pallet, Call, Storage, Origin<T>, Event<T>, Config<T>} = 15,
		PhragmenElection: pallet_elections_phragmen::{Pallet, Call, Storage, Event<T>, Config<T>} = 16,
		TechnicalMembership: pallet_membership::<Instance1>::{Pallet, Call, Storage, Event<T>, Config<T>} = 17,
		Treasury: pallet_treasury::{Pallet, Call, Storage, Config<T>, Event<T>} = 18,

		// Claims. Usable initially.
		Claims: claims::{Pallet, Call, Storage, Event<T>, Config<T>, ValidateUnsigned} = 19,

		// Utility module.
		Utility: pallet_utility::{Pallet, Call, Event} = 24,

		// Less simple identity module.
		Identity: pallet_identity::{Pallet, Call, Storage, Event<T>} = 25,

		// Society module.
		Society: pallet_society::{Pallet, Call, Storage, Event<T>} = 26,

		// Social recovery module.
		Recovery: pallet_recovery::{Pallet, Call, Storage, Event<T>} = 27,

		// Vesting. Usable initially, but removed once all vesting is finished.
		Vesting: pallet_vesting::{Pallet, Call, Storage, Event<T>, Config<T>} = 28,

		// System scheduler.
		Scheduler: pallet_scheduler::{Pallet, Call, Storage, Event<T>} = 29,

		// Proxy module. Late addition.
		Proxy: pallet_proxy::{Pallet, Call, Storage, Event<T>} = 30,

		// Multisig module. Late addition.
		Multisig: pallet_multisig::{Pallet, Call, Storage, Event<T>} = 31,

		// Preimage registrar.
		Preimage: pallet_preimage::{Pallet, Call, Storage, Event<T>} = 32,

		// Bounties modules.
		Bounties: pallet_bounties::{Pallet, Call, Storage, Event<T>} = 35,
		ChildBounties: pallet_child_bounties = 40,

		// Tips module.
		Tips: pallet_tips::{Pallet, Call, Storage, Event<T>} = 36,

		// NIS pallet.
		Nis: pallet_nis::{Pallet, Call, Storage, Event<T>, HoldReason} = 38,
//		pub type NisCounterpartInstance = pallet_balances::Instance2;
		NisCounterpartBalances: pallet_balances::<Instance2> = 45,

		// Parachains pallets. Start indices at 50 to leave room.
		ParachainsOrigin: parachains_origin::{Pallet, Origin} = 50,
		Configuration: parachains_configuration::{Pallet, Call, Storage, Config<T>} = 51,
		ParasShared: parachains_shared::{Pallet, Call, Storage} = 52,
		ParaInclusion: parachains_inclusion::{Pallet, Call, Storage, Event<T>} = 53,
		ParaInherent: parachains_paras_inherent::{Pallet, Call, Storage, Inherent} = 54,
		ParaScheduler: parachains_scheduler::{Pallet, Storage} = 55,
		Paras: parachains_paras::{Pallet, Call, Storage, Event, Config<T>, ValidateUnsigned} = 56,
		Initializer: parachains_initializer::{Pallet, Call, Storage} = 57,
		Dmp: parachains_dmp::{Pallet, Storage} = 58,
		Hrmp: parachains_hrmp::{Pallet, Call, Storage, Event<T>, Config<T>} = 60,
		ParaSessionInfo: parachains_session_info::{Pallet, Storage} = 61,
		ParasDisputes: parachains_disputes::{Pallet, Call, Storage, Event<T>} = 62,
		ParasSlashing: parachains_slashing::{Pallet, Call, Storage, ValidateUnsigned} = 63,
		MessageQueue: pallet_message_queue::{Pallet, Call, Storage, Event<T>} = 64,
		ParaAssignmentProvider: parachains_assigner::{Pallet, Storage} = 65,
		OnDemandAssignmentProvider: parachains_assigner_on_demand::{Pallet, Call, Storage, Event<T>} = 66,
		ParachainsAssignmentProvider: parachains_assigner_parachains::{Pallet} = 67,

		// Parachain Onboarding Pallets. Start indices at 70 to leave room.
		Registrar: paras_registrar::{Pallet, Call, Storage, Event<T>, Config<T>} = 70,
		Slots: slots::{Pallet, Call, Storage, Event<T>} = 71,
		Auctions: auctions::{Pallet, Call, Storage, Event<T>} = 72,
		Crowdloan: crowdloan::{Pallet, Call, Storage, Event<T>} = 73,

		// Pallet for sending XCM.
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>, Origin, Config<T>} = 99,

		ParasSudoWrapper: paras_sudo_wrapper::{Pallet, Call} = 250,
		AssignedSlots: assigned_slots::{Pallet, Call, Storage, Event<T>, Config<T>} = 251,

		// Validator Manager pallet.
		ValidatorManager: validator_manager::{Pallet, Call, Storage, Event<T>} = 252,

		// State trie migration pallet, only temporary.
		StateTrieMigration: pallet_state_trie_migration = 254,

		// Sudo.
		Sudo: pallet_sudo::{Pallet, Call, Storage, Event<T>, Config<T>} = 255,
	}
}

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// `BlockId` type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The `SignedExtension` to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckMortality<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;

/// All migrations that will run on the next runtime upgrade.
///
/// This contains the combined migrations of the last 10 releases. It allows to skip runtime
/// upgrades in case governance decides to do so. THE ORDER IS IMPORTANT.
pub type Migrations = migrations::Unreleased;

/// The runtime migrations per release.
#[allow(deprecated, missing_docs)]
pub mod migrations {
	use super::*;

	/// Unreleased migrations. Add new ones here:
	pub type Unreleased = (
		pallet_society::migrations::VersionCheckedMigrateToV2<Runtime, (), ()>,
		pallet_im_online::migration::v1::Migration<Runtime>,
		parachains_configuration::migration::v7::MigrateToV7<Runtime>,
		assigned_slots::migration::v1::VersionCheckedMigrateToV1<Runtime>,
		parachains_scheduler::migration::v1::MigrateToV1<Runtime>,
		parachains_configuration::migration::v8::MigrateToV8<Runtime>,
	);
}

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
	Migrations,
>;
/// The payload being signed in transactions.
pub type SignedPayload = generic::SignedPayload<RuntimeCall, SignedExtra>;

parameter_types! {
	// The deposit configuration for the singed migration. Specially if you want to allow any signed account to do the migration (see `SignedFilter`, these deposits should be high)
	pub const MigrationSignedDepositPerItem: Balance = 1 * CENTS;
	pub const MigrationSignedDepositBase: Balance = 20 * CENTS * 100;
	pub const MigrationMaxKeyLen: u32 = 512;
}

impl pallet_state_trie_migration::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type SignedDepositPerItem = MigrationSignedDepositPerItem;
	type SignedDepositBase = MigrationSignedDepositBase;
	type ControlOrigin = EnsureRoot<AccountId>;
	// specific account for the migration, can trigger the signed migrations.
	type SignedFilter = frame_system::EnsureSignedBy<MigController, AccountId>;

	// Use same weights as substrate ones.
	type WeightInfo = pallet_state_trie_migration::weights::SubstrateWeight<Runtime>;
	type MaxKeyLen = MigrationMaxKeyLen;
}

frame_support::ord_parameter_types! {
	pub const MigController: AccountId = AccountId::from(hex_literal::hex!("52bc71c1eca5353749542dfdf0af97bf764f9c2f44e860cd485f1cd86400f649"));
}

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	frame_benchmarking::define_benchmarks!(
		// Polkadot
		// NOTE: Make sure to prefix these with `runtime_common::` so
		// the that path resolves correctly in the generated file.
		[runtime_common::assigned_slots, AssignedSlots]
		[runtime_common::auctions, Auctions]
		[runtime_common::crowdloan, Crowdloan]
		[runtime_common::claims, Claims]
		[runtime_common::slots, Slots]
		[runtime_common::paras_registrar, Registrar]
		[runtime_parachains::configuration, Configuration]
		[runtime_parachains::hrmp, Hrmp]
		[runtime_parachains::disputes, ParasDisputes]
		[runtime_parachains::inclusion, ParaInclusion]
		[runtime_parachains::initializer, Initializer]
		[runtime_parachains::paras_inherent, ParaInherent]
		[runtime_parachains::paras, Paras]
		[runtime_parachains::assigner_on_demand, OnDemandAssignmentProvider]
		// Substrate
		[pallet_balances, Balances]
		[pallet_balances, NisCounterpartBalances]
		[frame_benchmarking::baseline, Baseline::<Runtime>]
		[pallet_bounties, Bounties]
		[pallet_child_bounties, ChildBounties]
		[pallet_collective, Council]
		[pallet_collective, TechnicalCommittee]
		[pallet_democracy, Democracy]
		[pallet_elections_phragmen, PhragmenElection]
		[pallet_nis, Nis]
		[pallet_identity, Identity]
		[pallet_im_online, ImOnline]
		[pallet_indices, Indices]
		[pallet_membership, TechnicalMembership]
		[pallet_message_queue, MessageQueue]
		[pallet_multisig, Multisig]
		[pallet_preimage, Preimage]
		[pallet_proxy, Proxy]
		[pallet_recovery, Recovery]
		[pallet_scheduler, Scheduler]
		[pallet_sudo, Sudo]
		[frame_system, SystemBench::<Runtime>]
		[pallet_timestamp, Timestamp]
		[pallet_tips, Tips]
		[pallet_treasury, Treasury]
		[pallet_utility, Utility]
		[pallet_vesting, Vesting]
		// XCM
		[pallet_xcm, XcmPallet]
		[pallet_xcm_benchmarks::fungible, pallet_xcm_benchmarks::fungible::Pallet::<Runtime>]
		[pallet_xcm_benchmarks::generic, pallet_xcm_benchmarks::generic::Pallet::<Runtime>]
	);
}

#[cfg(not(feature = "disable-runtime-api"))]
sp_api::impl_runtime_apis! {
	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block);
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}

		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> sp_std::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl block_builder_api::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: inherents::InherentData,
		) -> inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl tx_pool_api::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl offchain_primitives::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl primitives::runtime_api::ParachainHost<Block, Hash, BlockNumber> for Runtime {
		fn validators() -> Vec<ValidatorId> {
			parachains_runtime_api_impl::validators::<Runtime>()
		}

		fn validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>) {
			parachains_runtime_api_impl::validator_groups::<Runtime>()
		}

		fn availability_cores() -> Vec<CoreState<Hash, BlockNumber>> {
			parachains_runtime_api_impl::availability_cores::<Runtime>()
		}

		fn persisted_validation_data(para_id: ParaId, assumption: OccupiedCoreAssumption)
			-> Option<PersistedValidationData<Hash, BlockNumber>> {
			parachains_runtime_api_impl::persisted_validation_data::<Runtime>(para_id, assumption)
		}

		fn assumed_validation_data(
			para_id: ParaId,
			expected_persisted_validation_data_hash: Hash,
		) -> Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)> {
			parachains_runtime_api_impl::assumed_validation_data::<Runtime>(
				para_id,
				expected_persisted_validation_data_hash,
			)
		}

		fn check_validation_outputs(
			para_id: ParaId,
			outputs: primitives::CandidateCommitments,
		) -> bool {
			parachains_runtime_api_impl::check_validation_outputs::<Runtime>(para_id, outputs)
		}

		fn session_index_for_child() -> SessionIndex {
			parachains_runtime_api_impl::session_index_for_child::<Runtime>()
		}

		fn validation_code(para_id: ParaId, assumption: OccupiedCoreAssumption)
			-> Option<ValidationCode> {
			parachains_runtime_api_impl::validation_code::<Runtime>(para_id, assumption)
		}

		fn candidate_pending_availability(para_id: ParaId) -> Option<CommittedCandidateReceipt<Hash>> {
			parachains_runtime_api_impl::candidate_pending_availability::<Runtime>(para_id)
		}

		fn candidate_events() -> Vec<CandidateEvent<Hash>> {
			parachains_runtime_api_impl::candidate_events::<Runtime, _>(|ev| {
				match ev {
					RuntimeEvent::ParaInclusion(ev) => {
						Some(ev)
					}
					_ => None,
				}
			})
		}

		fn session_info(index: SessionIndex) -> Option<SessionInfo> {
			parachains_runtime_api_impl::session_info::<Runtime>(index)
		}

		fn session_executor_params(session_index: SessionIndex) -> Option<ExecutorParams> {
			parachains_runtime_api_impl::session_executor_params::<Runtime>(session_index)
		}

		fn dmq_contents(recipient: ParaId) -> Vec<InboundDownwardMessage<BlockNumber>> {
			parachains_runtime_api_impl::dmq_contents::<Runtime>(recipient)
		}

		fn inbound_hrmp_channels_contents(
			recipient: ParaId
		) -> BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>> {
			parachains_runtime_api_impl::inbound_hrmp_channels_contents::<Runtime>(recipient)
		}

		fn validation_code_by_hash(hash: ValidationCodeHash) -> Option<ValidationCode> {
			parachains_runtime_api_impl::validation_code_by_hash::<Runtime>(hash)
		}

		fn on_chain_votes() -> Option<ScrapedOnChainVotes<Hash>> {
			parachains_runtime_api_impl::on_chain_votes::<Runtime>()
		}

		fn submit_pvf_check_statement(
			stmt: primitives::PvfCheckStatement,
			signature: primitives::ValidatorSignature
		) {
			parachains_runtime_api_impl::submit_pvf_check_statement::<Runtime>(stmt, signature)
		}

		fn pvfs_require_precheck() -> Vec<ValidationCodeHash> {
			parachains_runtime_api_impl::pvfs_require_precheck::<Runtime>()
		}

		fn validation_code_hash(para_id: ParaId, assumption: OccupiedCoreAssumption)
			-> Option<ValidationCodeHash>
		{
			parachains_runtime_api_impl::validation_code_hash::<Runtime>(para_id, assumption)
		}

		fn disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)> {
			parachains_runtime_api_impl::get_session_disputes::<Runtime>()
		}

		fn unapplied_slashes(
		) -> Vec<(SessionIndex, CandidateHash, slashing::PendingSlashes)> {
			parachains_runtime_api_impl::unapplied_slashes::<Runtime>()
		}

		fn key_ownership_proof(
			validator_id: ValidatorId,
		) -> Option<slashing::OpaqueKeyOwnershipProof> {
			use parity_scale_codec::Encode;

			Historical::prove((PARACHAIN_KEY_TYPE_ID, validator_id))
				.map(|p| p.encode())
				.map(slashing::OpaqueKeyOwnershipProof::new)
		}

		fn submit_report_dispute_lost(
			dispute_proof: slashing::DisputeProof,
			key_ownership_proof: slashing::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			parachains_runtime_api_impl::submit_unsigned_slashing_report::<Runtime>(
				dispute_proof,
				key_ownership_proof,
			)
		}
	}

	#[api_version(3)]
	impl beefy_primitives::BeefyApi<Block, BeefyId> for Runtime {
		fn beefy_genesis() -> Option<BlockNumber> {
			Beefy::genesis_block()
		}

		fn validator_set() -> Option<beefy_primitives::ValidatorSet<BeefyId>> {
			Beefy::validator_set()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			equivocation_proof: beefy_primitives::EquivocationProof<
				BlockNumber,
				BeefyId,
				BeefySignature,
			>,
			key_owner_proof: beefy_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			let key_owner_proof = key_owner_proof.decode()?;

			Beefy::submit_unsigned_equivocation_report(
				equivocation_proof,
				key_owner_proof,
			)
		}

		fn generate_key_ownership_proof(
			_set_id: beefy_primitives::ValidatorSetId,
			authority_id: BeefyId,
		) -> Option<beefy_primitives::OpaqueKeyOwnershipProof> {
			use parity_scale_codec::Encode;

			Historical::prove((beefy_primitives::KEY_TYPE, authority_id))
				.map(|p| p.encode())
				.map(beefy_primitives::OpaqueKeyOwnershipProof::new)
		}
	}

	#[api_version(2)]
	impl mmr::MmrApi<Block, mmr::Hash, BlockNumber> for Runtime {
		fn mmr_root() -> Result<mmr::Hash, mmr::Error> {
			Ok(Mmr::mmr_root())
		}

		fn mmr_leaf_count() -> Result<mmr::LeafIndex, mmr::Error> {
			Ok(Mmr::mmr_leaves())
		}

		fn generate_proof(
			block_numbers: Vec<BlockNumber>,
			best_known_block_number: Option<BlockNumber>,
		) -> Result<(Vec<mmr::EncodableOpaqueLeaf>, mmr::Proof<mmr::Hash>), mmr::Error> {
			Mmr::generate_proof(block_numbers, best_known_block_number).map(
				|(leaves, proof)| {
					(
						leaves
							.into_iter()
							.map(|leaf| mmr::EncodableOpaqueLeaf::from_leaf(&leaf))
							.collect(),
						proof,
					)
				},
			)
		}

		fn verify_proof(leaves: Vec<mmr::EncodableOpaqueLeaf>, proof: mmr::Proof<mmr::Hash>)
			-> Result<(), mmr::Error>
		{
			let leaves = leaves.into_iter().map(|leaf|
				leaf.into_opaque_leaf()
				.try_decode()
				.ok_or(mmr::Error::Verify)).collect::<Result<Vec<mmr::Leaf>, mmr::Error>>()?;
			Mmr::verify_leaves(leaves, proof)
		}

		fn verify_proof_stateless(
			root: mmr::Hash,
			leaves: Vec<mmr::EncodableOpaqueLeaf>,
			proof: mmr::Proof<mmr::Hash>
		) -> Result<(), mmr::Error> {
			let nodes = leaves.into_iter().map(|leaf|mmr::DataOrHash::Data(leaf.into_opaque_leaf())).collect();
			pallet_mmr::verify_leaves_proof::<mmr::Hashing, _>(root, nodes, proof)
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> Vec<(GrandpaId, u64)> {
			Grandpa::grandpa_authorities()
		}

		fn current_set_id() -> fg_primitives::SetId {
			Grandpa::current_set_id()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			equivocation_proof: fg_primitives::EquivocationProof<
				<Block as BlockT>::Hash,
				sp_runtime::traits::NumberFor<Block>,
			>,
			key_owner_proof: fg_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			let key_owner_proof = key_owner_proof.decode()?;

			Grandpa::submit_unsigned_equivocation_report(
				equivocation_proof,
				key_owner_proof,
			)
		}

		fn generate_key_ownership_proof(
			_set_id: fg_primitives::SetId,
			authority_id: fg_primitives::AuthorityId,
		) -> Option<fg_primitives::OpaqueKeyOwnershipProof> {
			use parity_scale_codec::Encode;

			Historical::prove((fg_primitives::KEY_TYPE, authority_id))
				.map(|p| p.encode())
				.map(fg_primitives::OpaqueKeyOwnershipProof::new)
		}
	}

	impl babe_primitives::BabeApi<Block> for Runtime {
		fn configuration() -> babe_primitives::BabeConfiguration {
			let epoch_config = Babe::epoch_config().unwrap_or(BABE_GENESIS_EPOCH_CONFIG);
			babe_primitives::BabeConfiguration {
				slot_duration: Babe::slot_duration(),
				epoch_length: EpochDurationInBlocks::get().into(),
				c: epoch_config.c,
				authorities: Babe::authorities().to_vec(),
				randomness: Babe::randomness(),
				allowed_slots: epoch_config.allowed_slots,
			}
		}

		fn current_epoch_start() -> babe_primitives::Slot {
			Babe::current_epoch_start()
		}

		fn current_epoch() -> babe_primitives::Epoch {
			Babe::current_epoch()
		}

		fn next_epoch() -> babe_primitives::Epoch {
			Babe::next_epoch()
		}

		fn generate_key_ownership_proof(
			_slot: babe_primitives::Slot,
			authority_id: babe_primitives::AuthorityId,
		) -> Option<babe_primitives::OpaqueKeyOwnershipProof> {
			use parity_scale_codec::Encode;

			Historical::prove((babe_primitives::KEY_TYPE, authority_id))
				.map(|p| p.encode())
				.map(babe_primitives::OpaqueKeyOwnershipProof::new)
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			equivocation_proof: babe_primitives::EquivocationProof<<Block as BlockT>::Header>,
			key_owner_proof: babe_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			let key_owner_proof = key_owner_proof.decode()?;

			Babe::submit_unsigned_equivocation_report(
				equivocation_proof,
				key_owner_proof,
			)
		}
	}

	impl authority_discovery_primitives::AuthorityDiscoveryApi<Block> for Runtime {
		fn authorities() -> Vec<AuthorityDiscoveryId> {
			parachains_runtime_api_impl::relevant_authority_ids::<Runtime>()
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, sp_core::crypto::KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<
		Block,
		Balance,
	> for Runtime {
		fn query_info(uxt: <Block as BlockT>::Extrinsic, len: u32) -> RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(uxt: <Block as BlockT>::Extrinsic, len: u32) -> FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl pallet_beefy_mmr::BeefyMmrApi<Block, Hash> for RuntimeApi {
		fn authority_set_proof() -> beefy_primitives::mmr::BeefyAuthoritySet<Hash> {
			MmrLeaf::authority_set_proof()
		}

		fn next_authority_set_proof() -> beefy_primitives::mmr::BeefyNextAuthoritySet<Hash> {
			MmrLeaf::next_authority_set_proof()
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			log::info!("try-runtime::on_runtime_upgrade rococo.");
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, BlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect,
		) -> Weight {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here.
			Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;

			use frame_system_benchmarking::Pallet as SystemBench;
			use frame_benchmarking::baseline::Pallet as Baseline;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			return (list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig,
		) -> Result<
			Vec<frame_benchmarking::BenchmarkBatch>,
			sp_runtime::RuntimeString,
		> {
			use frame_support::traits::WhitelistedStorageKeys;
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, BenchmarkError};
			use frame_system_benchmarking::Pallet as SystemBench;
			use frame_benchmarking::baseline::Pallet as Baseline;
			use sp_storage::TrackedStorageKey;
			use xcm::latest::prelude::*;
			use xcm_config::{
				LocalCheckAccount, LocationConverter, Rockmine, TokenLocation, XcmConfig,
			};

			impl frame_system_benchmarking::Config for Runtime {}
			impl frame_benchmarking::baseline::Config for Runtime {}
			impl pallet_xcm_benchmarks::Config for Runtime {
				type XcmConfig = XcmConfig;
				type AccountIdConverter = LocationConverter;
				fn valid_destination() -> Result<MultiLocation, BenchmarkError> {
					Ok(Rockmine::get())
				}
				fn worst_case_holding(_depositable_count: u32) -> MultiAssets {
					// Rococo only knows about ROC
					vec![MultiAsset{
						id: Concrete(TokenLocation::get()),
						fun: Fungible(1_000_000 * UNITS),
					}].into()
				}
			}

			parameter_types! {
				pub const TrustedTeleporter: Option<(MultiLocation, MultiAsset)> = Some((
					Rockmine::get(),
					MultiAsset { fun: Fungible(1 * UNITS), id: Concrete(TokenLocation::get()) },
				));
				pub const TrustedReserve: Option<(MultiLocation, MultiAsset)> = None;
			}

			impl pallet_xcm_benchmarks::fungible::Config for Runtime {
				type TransactAsset = Balances;

				type CheckedAccount = LocalCheckAccount;
				type TrustedTeleporter = TrustedTeleporter;
				type TrustedReserve = TrustedReserve;

				fn get_multi_asset() -> MultiAsset {
					MultiAsset {
						id: Concrete(TokenLocation::get()),
						fun: Fungible(1 * UNITS),
					}
				}
			}

			impl pallet_xcm_benchmarks::generic::Config for Runtime {
				type RuntimeCall = RuntimeCall;

				fn worst_case_response() -> (u64, Response) {
					(0u64, Response::Version(Default::default()))
				}

				fn worst_case_asset_exchange() -> Result<(MultiAssets, MultiAssets), BenchmarkError> {
					// Rococo doesn't support asset exchanges
					Err(BenchmarkError::Skip)
				}

				fn universal_alias() -> Result<(MultiLocation, Junction), BenchmarkError> {
					// The XCM executor of Rococo doesn't have a configured `UniversalAliases`
					Err(BenchmarkError::Skip)
				}

				fn transact_origin_and_runtime_call() -> Result<(MultiLocation, RuntimeCall), BenchmarkError> {
					Ok((Rockmine::get(), frame_system::Call::remark_with_event { remark: vec![] }.into()))
				}

				fn subscribe_origin() -> Result<MultiLocation, BenchmarkError> {
					Ok(Rockmine::get())
				}

				fn claimable_asset() -> Result<(MultiLocation, MultiLocation, MultiAssets), BenchmarkError> {
					let origin = Rockmine::get();
					let assets: MultiAssets = (Concrete(TokenLocation::get()), 1_000 * UNITS).into();
					let ticket = MultiLocation { parents: 0, interior: Here };
					Ok((origin, ticket, assets))
				}

				fn unlockable_asset() -> Result<(MultiLocation, MultiLocation, MultiAsset), BenchmarkError> {
					// Rococo doesn't support asset locking
					Err(BenchmarkError::Skip)
				}

				fn export_message_origin_and_destination(
				) -> Result<(MultiLocation, NetworkId, InteriorMultiLocation), BenchmarkError> {
					// Rococo doesn't support exporting messages
					Err(BenchmarkError::Skip)
				}

				fn alias_origin() -> Result<(MultiLocation, MultiLocation), BenchmarkError> {
					// The XCM executor of Rococo doesn't have a configured `Aliasers`
					Err(BenchmarkError::Skip)
				}
			}

			let mut whitelist: Vec<TrackedStorageKey> = AllPalletsWithSystem::whitelisted_storage_keys();
			let treasury_key = frame_system::Account::<Runtime>::hashed_key_for(Treasury::account_id());
			whitelist.push(treasury_key.to_vec().into());

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);

			add_benchmarks!(params, batches);

			Ok(batches)
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashSet;

	use super::*;
	use frame_support::traits::WhitelistedStorageKeys;
	use sp_core::hexdisplay::HexDisplay;

	#[test]
	fn check_whitelist() {
		let whitelist: HashSet<String> = AllPalletsWithSystem::whitelisted_storage_keys()
			.iter()
			.map(|e| HexDisplay::from(&e.key).to_string())
			.collect();

		// Block number
		assert!(
			whitelist.contains("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac")
		);
		// Total issuance
		assert!(
			whitelist.contains("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80")
		);
		// Execution phase
		assert!(
			whitelist.contains("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a")
		);
		// Event count
		assert!(
			whitelist.contains("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850")
		);
		// System events
		assert!(
			whitelist.contains("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7")
		);
		// XcmPallet VersionDiscoveryQueue
		assert!(
			whitelist.contains("1405f2411d0af5a7ff397e7c9dc68d194a222ba0333561192e474c59ed8e30e1")
		);
		// XcmPallet SafeXcmVersion
		assert!(
			whitelist.contains("1405f2411d0af5a7ff397e7c9dc68d196323ae84c43568be0d1394d5d0d522c4")
		);
	}
}

#[cfg(test)]
mod encoding_tests {
	use super::*;

	#[test]
	fn nis_hold_reason_encoding_is_correct() {
		assert_eq!(RuntimeHoldReason::Nis(pallet_nis::HoldReason::NftReceipt).encode(), [38, 0]);
	}
}

#[cfg(all(test, feature = "try-runtime"))]
mod remote_tests {
	use super::*;
	use frame_try_runtime::{runtime_decl_for_try_runtime::TryRuntime, UpgradeCheckSelect};
	use remote_externalities::{
		Builder, Mode, OfflineConfig, OnlineConfig, SnapshotConfig, Transport,
	};
	use std::env::var;

	#[tokio::test]
	async fn run_migrations() {
		if var("RUN_MIGRATION_TESTS").is_err() {
			return
		}

		sp_tracing::try_init_simple();
		let transport: Transport =
			var("WS").unwrap_or("wss://rococo-rpc.polkadot.io:443".to_string()).into();
		let maybe_state_snapshot: Option<SnapshotConfig> = var("SNAP").map(|s| s.into()).ok();
		let mut ext = Builder::<Block>::default()
			.mode(if let Some(state_snapshot) = maybe_state_snapshot {
				Mode::OfflineOrElseOnline(
					OfflineConfig { state_snapshot: state_snapshot.clone() },
					OnlineConfig {
						transport,
						state_snapshot: Some(state_snapshot),
						..Default::default()
					},
				)
			} else {
				Mode::Online(OnlineConfig { transport, ..Default::default() })
			})
			.build()
			.await
			.unwrap();
		ext.execute_with(|| Runtime::on_runtime_upgrade(UpgradeCheckSelect::PreAndPost));
	}
}
