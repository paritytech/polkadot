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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! The Polkadot runtime. This can be compiled with `#[no_std]`, ready for Wasm.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

use pallet_transaction_payment::CurrencyAdapter;
use sp_std::prelude::*;
use sp_std::collections::btree_map::BTreeMap;
use sp_core::u32_trait::{_1, _2, _3, _5};
use parity_scale_codec::{Encode, Decode};
use primitives::v1::{
	AccountId, AccountIndex, Balance, BlockNumber, CandidateEvent, CommittedCandidateReceipt,
	CoreState, GroupRotationInfo, Hash, Id, Moment, Nonce, OccupiedCoreAssumption,
	PersistedValidationData, Signature, ValidationCode, ValidatorId, ValidatorIndex,
	InboundDownwardMessage, InboundHrmpMessage, SessionInfo, AssignmentId,
};
use runtime_common::{
	claims, SlowAdjustingFeeUpdate, CurrencyToVote,
	impls::DealWithFees,
	BlockHashCount, RocksDbWeight, BlockWeights, BlockLength, OffchainSolutionWeightLimit,
	ParachainSessionKeyPlaceholder, AssignmentSessionKeyPlaceholder,
};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys, ModuleId,
	ApplyExtrinsicResult, KeyTypeId, Percent, Permill, Perbill,
	transaction_validity::{TransactionValidity, TransactionSource, TransactionPriority},
	curve::PiecewiseLinear,
	traits::{
		BlakeTwo256, Block as BlockT, OpaqueKeys, ConvertInto, AccountIdLookup,
		Extrinsic as ExtrinsicT, SaturatedConversion, Verify,
	},
};
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::RuntimeString;
use sp_version::RuntimeVersion;
use pallet_grandpa::{AuthorityId as GrandpaId, fg_primitives};
#[cfg(any(feature = "std", test))]
use sp_version::NativeVersion;
use sp_core::OpaqueMetadata;
use sp_staking::SessionIndex;
use frame_support::{
	parameter_types, construct_runtime, debug, RuntimeDebug,
	traits::{KeyOwnerProofSystem, Randomness, LockIdentifier, Filter, InstanceFilter},
	weights::Weight,
};
use frame_system::{EnsureRoot, EnsureOneOf};
use pallet_im_online::sr25519::AuthorityId as ImOnlineId;
use authority_discovery_primitives::AuthorityId as AuthorityDiscoveryId;
use pallet_transaction_payment::{FeeDetails, RuntimeDispatchInfo};
use pallet_session::{historical as session_historical};
use static_assertions::const_assert;

#[cfg(feature = "std")]
pub use pallet_staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use pallet_timestamp::Call as TimestampCall;
pub use pallet_balances::Call as BalancesCall;

/// Constant values used within the runtime.
pub mod constants;
use constants::{time::*, currency::*, fee::*};

// Weights used in the runtime.
mod weights;

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/// Runtime version (Kusama).
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("kusama"),
	impl_name: create_runtime_str!("parity-kusama"),
	authoring_version: 2,
	spec_version: 2029,
	impl_version: 0,
	#[cfg(not(feature = "disable-runtime-api"))]
	apis: RUNTIME_API_VERSIONS,
	#[cfg(feature = "disable-runtime-api")]
	apis: version::create_apis_vec![[]],
	transaction_version: 4,
};

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion {
		runtime_version: VERSION,
		can_author_with: Default::default(),
	}
}

/// Avoid processing transactions from slots and parachain registrar.
pub struct BaseFilter;
impl Filter<Call> for BaseFilter {
	fn filter(_: &Call) -> bool {
		true
	}
}

type MoreThanHalfCouncil = EnsureOneOf<
	AccountId,
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>
>;

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub const SS58Prefix: u8 = 2;
}

impl frame_system::Config for Runtime {
	type BaseCallFilter = BaseFilter;
	type BlockWeights = BlockWeights;
	type BlockLength = BlockLength;
	type Origin = Origin;
	type Call = Call;
	type Index = Nonce;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Header = generic::Header<BlockNumber, BlakeTwo256>;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type DbWeight = RocksDbWeight;
	type Version = Version;
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	type SS58Prefix = SS58Prefix;
}

parameter_types! {
	pub MaximumSchedulerWeight: Weight = Perbill::from_percent(80) *
		BlockWeights::get().max_block;
	pub const MaxScheduledPerBlock: u32 = 50;
}

impl pallet_scheduler::Config for Runtime {
	type Event = Event;
	type Origin = Origin;
	type PalletsOrigin = OriginCaller;
	type Call = Call;
	type MaximumWeight = MaximumSchedulerWeight;
	type ScheduleOrigin = EnsureRoot<AccountId>;
	type MaxScheduledPerBlock = MaxScheduledPerBlock;
	type WeightInfo = weights::pallet_scheduler::WeightInfo<Runtime>;
}

parameter_types! {
	pub const EpochDuration: u64 = EPOCH_DURATION_IN_BLOCKS as u64;
	pub const ExpectedBlockTime: Moment = MILLISECS_PER_BLOCK;
	pub const ReportLongevity: u64 =
		BondingDuration::get() as u64 * SessionsPerEra::get() as u64 * EpochDuration::get();
}

impl pallet_babe::Config for Runtime {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;

	// session module is the trigger
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;

	type KeyOwnerProofSystem = Historical;

	type KeyOwnerProof = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::IdentificationTuple;

	type HandleEquivocation =
		pallet_babe::EquivocationHandler<Self::KeyOwnerIdentification, Offences, ReportLongevity>;

	type WeightInfo = ();
}

parameter_types! {
	pub const IndexDeposit: Balance = 1 * DOLLARS;
}

impl pallet_indices::Config for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type Event = Event;
	type WeightInfo = weights::pallet_indices::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 1 * CENTS;
	pub const MaxLocks: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type Balance = Balance;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type MaxLocks = MaxLocks;
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
}

parameter_types! {
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
}

impl pallet_transaction_payment::Config for Runtime {
	type OnChargeTransaction = CurrencyAdapter<Balances, DealWithFees<Self>>;
	type TransactionByteFee = TransactionByteFee;
	type WeightToFee = WeightToFee;
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

parameter_types! {
	pub const UncleGenerations: u32 = 0;
}

// TODO: substrate#2986 implement this properly
impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Babe>;
	type UncleGenerations = UncleGenerations;
	type FilterUncle = ();
	type EventHandler = (Staking, ImOnline);
}

parameter_types! {
	pub const Period: BlockNumber = 10 * MINUTES;
	pub const Offset: BlockNumber = 0;
}

impl_opaque_keys! {
	pub struct OldSessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub im_online: ImOnline,
		pub para_validator: ParachainSessionKeyPlaceholder<Runtime>,
		pub authority_discovery: AuthorityDiscovery,
	}
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub im_online: ImOnline,
		pub para_validator: ParachainSessionKeyPlaceholder<Runtime>,
		pub para_assignment: AssignmentSessionKeyPlaceholder<Runtime>,
		pub authority_discovery: AuthorityDiscovery,
	}
}

fn transform_session_keys(v: AccountId, old: OldSessionKeys) -> SessionKeys {
	SessionKeys {
		grandpa: old.grandpa,
		babe: old.babe,
		im_online: old.im_online,
		para_validator: old.para_validator,
		para_assignment: {
			// We need to produce a dummy value that's unique for the validator.
			let mut id = AssignmentId::default();
			let id_raw: &mut [u8] = id.as_mut();
			id_raw.copy_from_slice(v.as_ref());
			id_raw[0..4].copy_from_slice(b"asgn");

			id
		},
		authority_discovery: old.authority_discovery,
	}
}

parameter_types! {
	pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
}

impl pallet_session::Config for Runtime {
	type Event = Event;
	type ValidatorId = AccountId;
	type ValidatorIdOf = pallet_staking::StashOf<Self>;
	type ShouldEndSession = Babe;
	type NextSessionRotation = Babe;
	type SessionManager = pallet_session::historical::NoteHistoricalRoot<Self, Staking>;
	type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
}

impl pallet_session::historical::Config for Runtime {
	type FullIdentification = pallet_staking::Exposure<AccountId, Balance>;
	type FullIdentificationOf = pallet_staking::ExposureOf<Runtime>;
}

// TODO #6469: This shouldn't be static, but a lazily cached value, not built unless needed, and
// re-built in case input parameters have changed. The `ideal_stake` should be determined by the
// amount of parachain slots being bid on: this should be around `(75 - 25.min(slots / 4))%`.
pallet_staking_reward_curve::build! {
	const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
		min_inflation: 0_025_000,
		max_inflation: 0_100_000,
		// 3:2:1 staked : parachains : float.
		// while there's no parachains, then this is 75% staked : 25% float.
		ideal_stake: 0_750_000,
		falloff: 0_050_000,
		max_piece_count: 40,
		test_precision: 0_005_000,
	);
}

parameter_types! {
	// Six sessions in an era (6 hours).
	pub const SessionsPerEra: SessionIndex = 6;
	// 28 eras for unbonding (7 days).
	pub const BondingDuration: pallet_staking::EraIndex = 28;
	// 27 eras in which slashes can be cancelled (slightly less than 7 days).
	pub const SlashDeferDuration: pallet_staking::EraIndex = 27;
	pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	pub const MaxNominatorRewardedPerValidator: u32 = 128;
	// quarter of the last session will be for election.
	pub const ElectionLookahead: BlockNumber = EPOCH_DURATION_IN_BLOCKS / 4;
	pub const MaxIterations: u32 = 10;
	pub MinSolutionScoreBump: Perbill = Perbill::from_rational_approximation(5u32, 10_000);
}

type SlashCancelOrigin = EnsureOneOf<
	AccountId,
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>
>;

impl pallet_staking::Config for Runtime {
	type Currency = Balances;
	type UnixTime = Timestamp;
	type CurrencyToVote = CurrencyToVote;
	type RewardRemainder = Treasury;
	type Event = Event;
	type Slash = Treasury;
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SlashDeferDuration = SlashDeferDuration;
	// A majority of the council or root can cancel the slash.
	type SlashCancelOrigin = SlashCancelOrigin;
	type SessionInterface = Self;
	type RewardCurve = RewardCurve;
	type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
	type NextNewSession = Session;
	type ElectionLookahead = ElectionLookahead;
	type Call = Call;
	type UnsignedPriority = StakingUnsignedPriority;
	type MaxIterations = MaxIterations;
	type MinSolutionScoreBump = MinSolutionScoreBump;
	// The unsigned solution weight targeted by the OCW. We set it to the maximum possible value of
	// a single extrinsic.
	type OffchainSolutionWeightLimit = OffchainSolutionWeightLimit;
	type WeightInfo = weights::pallet_staking::WeightInfo<Runtime>;
}

parameter_types! {
	pub const LaunchPeriod: BlockNumber = 7 * DAYS;
	pub const VotingPeriod: BlockNumber = 7 * DAYS;
	pub const FastTrackVotingPeriod: BlockNumber = 3 * HOURS;
	pub const MinimumDeposit: Balance = 1 * DOLLARS;
	pub const EnactmentPeriod: BlockNumber = 8 * DAYS;
	pub const CooloffPeriod: BlockNumber = 7 * DAYS;
	// One cent: $10,000 / MB
	pub const PreimageByteDeposit: Balance = 10 * MILLICENTS;
	pub const InstantAllowed: bool = true;
	pub const MaxVotes: u32 = 100;
	pub const MaxProposals: u32 = 100;
}

impl pallet_democracy::Config for Runtime {
	type Proposal = Call;
	type Event = Event;
	type Currency = Balances;
	type EnactmentPeriod = EnactmentPeriod;
	type LaunchPeriod = LaunchPeriod;
	type VotingPeriod = VotingPeriod;
	type MinimumDeposit = MinimumDeposit;
	/// A straight majority of the council can decide what their next motion is.
	type ExternalOrigin = pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>;
	/// A majority can have the next scheduled referendum be a straight majority-carries vote.
	type ExternalMajorityOrigin = pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>;
	/// A unanimous council can have the next scheduled referendum be a straight default-carries
	/// (NTB) vote.
	type ExternalDefaultOrigin = pallet_collective::EnsureProportionAtLeast<_1, _1, AccountId, CouncilCollective>;
	/// Two thirds of the technical committee can have an ExternalMajority/ExternalDefault vote
	/// be tabled immediately and with a shorter voting/enactment period.
	type FastTrackOrigin = pallet_collective::EnsureProportionAtLeast<_2, _3, AccountId, TechnicalCollective>;
	type InstantOrigin = pallet_collective::EnsureProportionAtLeast<_1, _1, AccountId, TechnicalCollective>;
	type InstantAllowed = InstantAllowed;
	type FastTrackVotingPeriod = FastTrackVotingPeriod;
	// To cancel a proposal which has been passed, 2/3 of the council must agree to it.
	type CancellationOrigin = EnsureOneOf<
		AccountId,
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilCollective>,
	>;
	// To cancel a proposal before it has been passed, the technical committee must be unanimous or
	// Root must agree.
	type CancelProposalOrigin = EnsureOneOf<
		AccountId,
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<_1, _1, AccountId, TechnicalCollective>,
	>;
	type BlacklistOrigin = EnsureRoot<AccountId>;
	// Any single technical committee member may veto a coming council proposal, however they can
	// only do it once and it lasts only for the cooloff period.
	type VetoOrigin = pallet_collective::EnsureMember<AccountId, TechnicalCollective>;
	type CooloffPeriod = CooloffPeriod;
	type PreimageByteDeposit = PreimageByteDeposit;
	type Slash = Treasury;
	type Scheduler = Scheduler;
	type PalletsOrigin = OriginCaller;
	type MaxVotes = MaxVotes;
	type OperationalPreimageOrigin = pallet_collective::EnsureMember<AccountId, CouncilCollective>;
	type WeightInfo = weights::pallet_democracy::WeightInfo<Runtime>;
	type MaxProposals = MaxProposals;
}

parameter_types! {
	pub const CouncilMotionDuration: BlockNumber = 3 * DAYS;
	pub const CouncilMaxProposals: u32 = 100;
	pub const CouncilMaxMembers: u32 = 100;
}

type CouncilCollective = pallet_collective::Instance1;
impl pallet_collective::Config<CouncilCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
	type MotionDuration = CouncilMotionDuration;
	type MaxProposals = CouncilMaxProposals;
	type MaxMembers = CouncilMaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type WeightInfo = weights::pallet_collective::WeightInfo<Runtime>;
}

parameter_types! {
	pub const CandidacyBond: Balance = 1 * DOLLARS;
	// 1 storage item created, key size is 32 bytes, value size is 16+16.
	pub const VotingBondBase: Balance = deposit(1, 64);
	// additional data per vote is 32 bytes (account id).
	pub const VotingBondFactor: Balance = deposit(0, 32);
	/// Daily council elections
	pub const TermDuration: BlockNumber = 24 * HOURS;
	pub const DesiredMembers: u32 = 19;
	pub const DesiredRunnersUp: u32 = 19;
	pub const ElectionsPhragmenModuleId: LockIdentifier = *b"phrelect";
}

// Make sure that there are no more than MaxMembers members elected via phragmen.
const_assert!(DesiredMembers::get() <= CouncilMaxMembers::get());

impl pallet_elections_phragmen::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type ChangeMembers = Council;
	type InitializeMembers = Council;
	type CurrencyToVote = frame_support::traits::U128CurrencyToVote;
	type CandidacyBond = CandidacyBond;
	type VotingBondBase = VotingBondBase;
	type VotingBondFactor = VotingBondFactor;
	type LoserCandidate = Treasury;
	type KickedMember = Treasury;
	type DesiredMembers = DesiredMembers;
	type DesiredRunnersUp = DesiredRunnersUp;
	type TermDuration = TermDuration;
	type ModuleId = ElectionsPhragmenModuleId;
	type WeightInfo = weights::pallet_elections_phragmen::WeightInfo<Runtime>;
}

parameter_types! {
	pub const TechnicalMotionDuration: BlockNumber = 3 * DAYS;
	pub const TechnicalMaxProposals: u32 = 100;
	pub const TechnicalMaxMembers: u32 = 100;
}

type TechnicalCollective = pallet_collective::Instance2;
impl pallet_collective::Config<TechnicalCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
	type MotionDuration = TechnicalMotionDuration;
	type MaxProposals = TechnicalMaxProposals;
	type MaxMembers = TechnicalMaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type WeightInfo = weights::pallet_collective::WeightInfo<Runtime>;
}

impl pallet_membership::Config<pallet_membership::Instance1> for Runtime {
	type Event = Event;
	type AddOrigin = MoreThanHalfCouncil;
	type RemoveOrigin = MoreThanHalfCouncil;
	type SwapOrigin = MoreThanHalfCouncil;
	type ResetOrigin = MoreThanHalfCouncil;
	type PrimeOrigin = MoreThanHalfCouncil;
	type MembershipInitialized = TechnicalCommittee;
	type MembershipChanged = TechnicalCommittee;
}

parameter_types! {
	pub const ProposalBond: Permill = Permill::from_percent(5);
	pub const ProposalBondMinimum: Balance = 20 * DOLLARS;
	pub const SpendPeriod: BlockNumber = 6 * DAYS;
	pub const Burn: Permill = Permill::from_perthousand(2);
	pub const TreasuryModuleId: ModuleId = ModuleId(*b"py/trsry");

	pub const TipCountdown: BlockNumber = 1 * DAYS;
	pub const TipFindersFee: Percent = Percent::from_percent(20);
	pub const TipReportDepositBase: Balance = 1 * DOLLARS;
	pub const DataDepositPerByte: Balance = 1 * CENTS;
	pub const BountyDepositBase: Balance = 1 * DOLLARS;
	pub const BountyDepositPayoutDelay: BlockNumber = 4 * DAYS;
	pub const BountyUpdatePeriod: BlockNumber = 90 * DAYS;
	pub const MaximumReasonLength: u32 = 16384;
	pub const BountyCuratorDeposit: Permill = Permill::from_percent(50);
	pub const BountyValueMinimum: Balance = 2 * DOLLARS;
}

type ApproveOrigin = EnsureOneOf<
	AccountId,
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<_3, _5, AccountId, CouncilCollective>
>;

impl pallet_treasury::Config for Runtime {
	type ModuleId = TreasuryModuleId;
	type Currency = Balances;
	type ApproveOrigin = ApproveOrigin;
	type RejectOrigin = MoreThanHalfCouncil;
	type Event = Event;
	type OnSlash = Treasury;
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type SpendPeriod = SpendPeriod;
	type Burn = Burn;
	type BurnDestination = Society;
	type SpendFunds = Bounties;
	type WeightInfo = weights::pallet_treasury::WeightInfo<Runtime>;
}

impl pallet_bounties::Config for Runtime {
	type Event = Event;
	type BountyDepositBase = BountyDepositBase;
	type BountyDepositPayoutDelay = BountyDepositPayoutDelay;
	type BountyUpdatePeriod = BountyUpdatePeriod;
	type BountyCuratorDeposit = BountyCuratorDeposit;
	type BountyValueMinimum = BountyValueMinimum;
	type DataDepositPerByte = DataDepositPerByte;
	type MaximumReasonLength = MaximumReasonLength;
	type WeightInfo = weights::pallet_bounties::WeightInfo<Runtime>;

}

impl pallet_tips::Config for Runtime {
	type Event = Event;
	type DataDepositPerByte = DataDepositPerByte;
	type MaximumReasonLength = MaximumReasonLength;
	type Tippers = ElectionsPhragmen;
	type TipCountdown = TipCountdown;
	type TipFindersFee = TipFindersFee;
	type TipReportDepositBase = TipReportDepositBase;
	type WeightInfo = weights::pallet_tips::WeightInfo<Runtime>;
}

parameter_types! {
	pub OffencesWeightSoftLimit: Weight = Perbill::from_percent(60) * BlockWeights::get().max_block;
}

impl pallet_offences::Config for Runtime {
	type Event = Event;
	type IdentificationTuple = pallet_session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = Staking;
	type WeightSoftLimit = OffencesWeightSoftLimit;
}

impl pallet_authority_discovery::Config for Runtime {}

parameter_types! {
	pub const SessionDuration: BlockNumber = EPOCH_DURATION_IN_BLOCKS as _;
}

parameter_types! {
	pub StakingUnsignedPriority: TransactionPriority =
		Perbill::from_percent(90) * TransactionPriority::max_value();
	pub const ImOnlineUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl pallet_im_online::Config for Runtime {
	type AuthorityId = ImOnlineId;
	type Event = Event;
	type ValidatorSet = Historical;
	type ReportUnresponsiveness = Offences;
	type SessionDuration = SessionDuration;
	type UnsignedPriority = ImOnlineUnsignedPriority;
	type WeightInfo = weights::pallet_im_online::WeightInfo<Runtime>;
}

impl pallet_grandpa::Config for Runtime {
	type Event = Event;
	type Call = Call;

	type KeyOwnerProofSystem = Historical;

	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		GrandpaId,
	)>>::IdentificationTuple;

	type HandleEquivocation =
		pallet_grandpa::EquivocationHandler<Self::KeyOwnerIdentification, Offences, ReportLongevity>;

	type WeightInfo = ();
}

/// Submits transaction with the node's public and signature type. Adheres to the signed extension
/// format of the chain.
impl<LocalCall> frame_system::offchain::CreateSignedTransaction<LocalCall> for Runtime where
	Call: From<LocalCall>,
{
	fn create_transaction<C: frame_system::offchain::AppCrypto<Self::Public, Self::Signature>>(
		call: Call,
		public: <Signature as Verify>::Signer,
		account: AccountId,
		nonce: <Runtime as frame_system::Config>::Index,
	) -> Option<(Call, <UncheckedExtrinsic as ExtrinsicT>::SignaturePayload)> {
		use sp_runtime::traits::StaticLookup;
		// take the biggest period possible.
		let period = BlockHashCount::get()
			.checked_next_power_of_two()
			.map(|c| c / 2)
			.unwrap_or(2) as u64;

		let current_block = System::block_number()
			.saturated_into::<u64>()
			// The `System::block_number` is initialized with `n+1`,
			// so the actual block number is `n`.
			.saturating_sub(1);
		let tip = 0;
		let extra: SignedExtra = (
			frame_system::CheckSpecVersion::<Runtime>::new(),
			frame_system::CheckTxVersion::<Runtime>::new(),
			frame_system::CheckGenesis::<Runtime>::new(),
			frame_system::CheckMortality::<Runtime>::from(generic::Era::mortal(period, current_block)),
			frame_system::CheckNonce::<Runtime>::from(nonce),
			frame_system::CheckWeight::<Runtime>::new(),
			pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
		);
		let raw_payload = SignedPayload::new(call, extra).map_err(|e| {
			debug::warn!("Unable to create signed payload: {:?}", e);
		}).ok()?;
		let signature = raw_payload.using_encoded(|payload| {
			C::sign(payload, public)
		})?;
		let (call, extra, _) = raw_payload.deconstruct();
		let address = <Runtime as frame_system::Config>::Lookup::unlookup(account);
		Some((call, (address, signature, extra)))
	}
}

impl frame_system::offchain::SigningTypes for Runtime {
	type Public = <Signature as Verify>::Signer;
	type Signature = Signature;
}

impl<C> frame_system::offchain::SendTransactionTypes<C> for Runtime where
	Call: From<C>,
{
	type OverarchingCall = Call;
	type Extrinsic = UncheckedExtrinsic;
}

parameter_types! {
	pub Prefix: &'static [u8] = b"Pay KSMs to the Kusama account:";
}

impl claims::Config for Runtime {
	type Event = Event;
	type VestingSchedule = Vesting;
	type Prefix = Prefix;
	type MoveClaimOrigin = pallet_collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type WeightInfo = weights::runtime_common_claims::WeightInfo<Runtime>;
}

parameter_types! {
	// Minimum 100 bytes/KSM deposited (1 CENT/byte)
	pub const BasicDeposit: Balance = 10 * DOLLARS;       // 258 bytes on-chain
	pub const FieldDeposit: Balance = 250 * CENTS;        // 66 bytes on-chain
	pub const SubAccountDeposit: Balance = 2 * DOLLARS;   // 53 bytes on-chain
	pub const MaxSubAccounts: u32 = 100;
	pub const MaxAdditionalFields: u32 = 100;
	pub const MaxRegistrars: u32 = 20;
}

impl pallet_identity::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Slashed = Treasury;
	type BasicDeposit = BasicDeposit;
	type FieldDeposit = FieldDeposit;
	type SubAccountDeposit = SubAccountDeposit;
	type MaxSubAccounts = MaxSubAccounts;
	type MaxAdditionalFields = MaxAdditionalFields;
	type MaxRegistrars = MaxRegistrars;
	type RegistrarOrigin = MoreThanHalfCouncil;
	type ForceOrigin = MoreThanHalfCouncil;
	type WeightInfo = weights::pallet_identity::WeightInfo<Runtime>;
}

impl pallet_utility::Config for Runtime {
	type Event = Event;
	type Call = Call;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = deposit(0, 32);
	pub const MaxSignatories: u16 = 100;
}

impl pallet_multisig::Config for Runtime {
	type Event = Event;
	type Call = Call;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = MaxSignatories;
	type WeightInfo = weights::pallet_multisig::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ConfigDepositBase: Balance = 5 * DOLLARS;
	pub const FriendDepositFactor: Balance = 50 * CENTS;
	pub const MaxFriends: u16 = 9;
	pub const RecoveryDeposit: Balance = 5 * DOLLARS;
}

impl pallet_recovery::Config for Runtime {
	type Event = Event;
	type Call = Call;
	type Currency = Balances;
	type ConfigDepositBase = ConfigDepositBase;
	type FriendDepositFactor = FriendDepositFactor;
	type MaxFriends = MaxFriends;
	type RecoveryDeposit = RecoveryDeposit;
}

parameter_types! {
	pub const CandidateDeposit: Balance = 10 * DOLLARS;
	pub const WrongSideDeduction: Balance = 2 * DOLLARS;
	pub const MaxStrikes: u32 = 10;
	pub const RotationPeriod: BlockNumber = 80 * HOURS;
	pub const PeriodSpend: Balance = 500 * DOLLARS;
	pub const MaxLockDuration: BlockNumber = 36 * 30 * DAYS;
	pub const ChallengePeriod: BlockNumber = 7 * DAYS;
	pub const SocietyModuleId: ModuleId = ModuleId(*b"py/socie");
}

impl pallet_society::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Randomness = RandomnessCollectiveFlip;
	type CandidateDeposit = CandidateDeposit;
	type WrongSideDeduction = WrongSideDeduction;
	type MaxStrikes = MaxStrikes;
	type PeriodSpend = PeriodSpend;
	type MembershipChanged = ();
	type RotationPeriod = RotationPeriod;
	type MaxLockDuration = MaxLockDuration;
	type FounderSetOrigin = pallet_collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type SuspensionJudgementOrigin = pallet_society::EnsureFounder<Runtime>;
	type ChallengePeriod = ChallengePeriod;
	type ModuleId = SocietyModuleId;
}

parameter_types! {
	pub const MinVestedTransfer: Balance = 100 * DOLLARS;
}

impl pallet_vesting::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type BlockNumberToBalance = ConvertInto;
	type MinVestedTransfer = MinVestedTransfer;
	type WeightInfo = weights::pallet_vesting::WeightInfo<Runtime>;
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
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum ProxyType {
	Any,
	NonTransfer,
	Governance,
	Staking,
	IdentityJudgement,
	CancelProxy,
}
impl Default for ProxyType { fn default() -> Self { Self::Any } }
impl InstanceFilter<Call> for ProxyType {
	fn filter(&self, c: &Call) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::NonTransfer => matches!(c,
				Call::System(..) |
				Call::Babe(..) |
				Call::Timestamp(..) |
				Call::Indices(pallet_indices::Call::claim(..)) |
				Call::Indices(pallet_indices::Call::free(..)) |
				Call::Indices(pallet_indices::Call::freeze(..)) |
				// Specifically omitting Indices `transfer`, `force_transfer`
				// Specifically omitting the entire Balances pallet
				Call::Authorship(..) |
				Call::Staking(..) |
				Call::Offences(..) |
				Call::Session(..) |
				Call::Grandpa(..) |
				Call::ImOnline(..) |
				Call::AuthorityDiscovery(..) |
				Call::Democracy(..) |
				Call::Council(..) |
				Call::TechnicalCommittee(..) |
				Call::ElectionsPhragmen(..) |
				Call::TechnicalMembership(..) |
				Call::Treasury(..) |
				Call::Bounties(..) |
				Call::Tips(..) |
				Call::Claims(..) |
				Call::Utility(..) |
				Call::Identity(..) |
				Call::Society(..) |
				Call::Recovery(pallet_recovery::Call::as_recovered(..)) |
				Call::Recovery(pallet_recovery::Call::vouch_recovery(..)) |
				Call::Recovery(pallet_recovery::Call::claim_recovery(..)) |
				Call::Recovery(pallet_recovery::Call::close_recovery(..)) |
				Call::Recovery(pallet_recovery::Call::remove_recovery(..)) |
				Call::Recovery(pallet_recovery::Call::cancel_recovered(..)) |
				// Specifically omitting Recovery `create_recovery`, `initiate_recovery`
				Call::Vesting(pallet_vesting::Call::vest(..)) |
				Call::Vesting(pallet_vesting::Call::vest_other(..)) |
				// Specifically omitting Vesting `vested_transfer`, and `force_vested_transfer`
				Call::Scheduler(..) |
				Call::Proxy(..) |
				Call::Multisig(..)
			),
			ProxyType::Governance => matches!(c,
				Call::Democracy(..) |
				Call::Council(..) |
				Call::TechnicalCommittee(..) |
				Call::ElectionsPhragmen(..) |
				Call::Treasury(..) |
				Call::Bounties(..) |
				Call::Tips(..) |
				Call::Utility(..)
			),
			ProxyType::Staking => matches!(c,
				Call::Staking(..) |
				Call::Session(..) |
				Call::Utility(..)
			),
			ProxyType::IdentityJudgement => matches!(c,
				Call::Identity(pallet_identity::Call::provide_judgement(..)) |
				Call::Utility(..)
			),
			ProxyType::CancelProxy => matches!(c,
				Call::Proxy(pallet_proxy::Call::reject_announcement(..))
			)
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
	type Event = Event;
	type Call = Call;
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

// When this is removed, should also remove `OldSessionKeys`.
pub struct UpgradeSessionKeys;
impl frame_support::traits::OnRuntimeUpgrade for UpgradeSessionKeys {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		Session::upgrade_keys::<OldSessionKeys, _>(transform_session_keys);
		Perbill::from_percent(50) * BlockWeights::get().max_block
	}
}

pub struct PhragmenElectionDepositRuntimeUpgrade;
impl pallet_elections_phragmen::migrations_3_0_0::V2ToV3 for PhragmenElectionDepositRuntimeUpgrade {
	type AccountId = AccountId;
	type Balance = Balance;
	type Module = ElectionsPhragmen;
}
impl frame_support::traits::OnRuntimeUpgrade for PhragmenElectionDepositRuntimeUpgrade {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		pallet_elections_phragmen::migrations_3_0_0::apply::<Self>(5 * CENTS, DOLLARS)
	}
}

construct_runtime! {
	pub enum Runtime where
		Block = Block,
		NodeBlock = primitives::v1::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		// Basic stuff; balances is uncallable initially.
		System: frame_system::{Module, Call, Storage, Config, Event<T>} = 0,
		RandomnessCollectiveFlip: pallet_randomness_collective_flip::{Module, Storage} = 32,

		// Must be before session.
		Babe: pallet_babe::{Module, Call, Storage, Config, ValidateUnsigned} = 1,

		Timestamp: pallet_timestamp::{Module, Call, Storage, Inherent} = 2,
		Indices: pallet_indices::{Module, Call, Storage, Config<T>, Event<T>} = 3,
		Balances: pallet_balances::{Module, Call, Storage, Config<T>, Event<T>} = 4,
		TransactionPayment: pallet_transaction_payment::{Module, Storage} = 33,

		// Consensus support.
		Authorship: pallet_authorship::{Module, Call, Storage} = 5,
		Staking: pallet_staking::{Module, Call, Storage, Config<T>, Event<T>, ValidateUnsigned} = 6,
		Offences: pallet_offences::{Module, Call, Storage, Event} = 7,
		Historical: session_historical::{Module} = 34,
		Session: pallet_session::{Module, Call, Storage, Event, Config<T>} = 8,
		Grandpa: pallet_grandpa::{Module, Call, Storage, Config, Event, ValidateUnsigned} = 10,
		ImOnline: pallet_im_online::{Module, Call, Storage, Event<T>, ValidateUnsigned, Config<T>} = 11,
		AuthorityDiscovery: pallet_authority_discovery::{Module, Call, Config} = 12,

		// Governance stuff; uncallable initially.
		Democracy: pallet_democracy::{Module, Call, Storage, Config, Event<T>} = 13,
		Council: pallet_collective::<Instance1>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>} = 14,
		TechnicalCommittee: pallet_collective::<Instance2>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>} = 15,
		ElectionsPhragmen: pallet_elections_phragmen::{Module, Call, Storage, Event<T>, Config<T>} = 16,
		TechnicalMembership: pallet_membership::<Instance1>::{Module, Call, Storage, Event<T>, Config<T>} = 17,
		Treasury: pallet_treasury::{Module, Call, Storage, Config, Event<T>} = 18,

		// Claims. Usable initially.
		Claims: claims::{Module, Call, Storage, Event<T>, Config<T>, ValidateUnsigned} = 19,

		// Utility module.
		Utility: pallet_utility::{Module, Call, Event} = 24,

		// Less simple identity module.
		Identity: pallet_identity::{Module, Call, Storage, Event<T>} = 25,

		// Society module.
		Society: pallet_society::{Module, Call, Storage, Event<T>} = 26,

		// Social recovery module.
		Recovery: pallet_recovery::{Module, Call, Storage, Event<T>} = 27,

		// Vesting. Usable initially, but removed once all vesting is finished.
		Vesting: pallet_vesting::{Module, Call, Storage, Event<T>, Config<T>} = 28,

		// System scheduler.
		Scheduler: pallet_scheduler::{Module, Call, Storage, Event<T>} = 29,

		// Proxy module. Late addition.
		Proxy: pallet_proxy::{Module, Call, Storage, Event<T>} = 30,

		// Multisig module. Late addition.
		Multisig: pallet_multisig::{Module, Call, Storage, Event<T>} = 31,

		// Bounties module.
		Bounties: pallet_bounties::{Module, Call, Storage, Event<T>} = 35,

		// Tips module.
		Tips: pallet_tips::{Module, Call, Storage, Event<T>} = 36,
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
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckMortality<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<Address, Call, Signature, SignedExtra>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Nonce, Call>;
/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllModules,
	(UpgradeSessionKeys, PhragmenElectionDepositRuntimeUpgrade)
>;
/// The payload being signed in the transactions.
pub type SignedPayload = generic::SignedPayload<Call, SignedExtra>;

#[cfg(not(feature = "disable-runtime-api"))]
sp_api::impl_runtime_apis! {
	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			Runtime::metadata().into()
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

		fn random_seed() -> <Block as BlockT>::Hash {
			RandomnessCollectiveFlip::random_seed()
		}
	}

	impl tx_pool_api::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx)
		}
	}

	impl offchain_primitives::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl primitives::v1::ParachainHost<Block, Hash, BlockNumber> for Runtime {
		fn validators() -> Vec<ValidatorId> {
			Vec::new()
		}

		fn validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>) {
			(Vec::new(), GroupRotationInfo { session_start_block: 0, group_rotation_frequency: 0, now: 0 })
		}

		fn availability_cores() -> Vec<CoreState<Hash, BlockNumber>> {
			Vec::new()
		}

		fn persisted_validation_data(_: Id, _: OccupiedCoreAssumption)
			-> Option<PersistedValidationData<BlockNumber>> {
			None
		}
		fn check_validation_outputs(
			_: Id,
			_: primitives::v1::CandidateCommitments,
		) -> bool {
			false
		}

		fn session_index_for_child() -> SessionIndex {
			0
		}

		fn session_info(_: SessionIndex) -> Option<SessionInfo> {
			None
		}

		fn validation_code(_: Id, _: OccupiedCoreAssumption) -> Option<ValidationCode> {
			None
		}

		fn historical_validation_code(_: Id, _: BlockNumber) -> Option<ValidationCode> {
			None
		}

		fn candidate_pending_availability(_: Id) -> Option<CommittedCandidateReceipt<Hash>> {
			None
		}

		fn candidate_events() -> Vec<CandidateEvent<Hash>> {
			Vec::new()
		}

		fn dmq_contents(
			_recipient: Id,
		) -> Vec<InboundDownwardMessage<BlockNumber>> {
			Vec::new()
		}

		fn inbound_hrmp_channels_contents(
			_recipient: Id
		) -> BTreeMap<Id, Vec<InboundHrmpMessage<BlockNumber>>> {
			BTreeMap::new()
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> Vec<(GrandpaId, u64)> {
			Grandpa::grandpa_authorities()
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
		fn configuration() -> babe_primitives::BabeGenesisConfiguration {
			// The choice of `c` parameter (where `1 - c` represents the
			// probability of a slot being empty), is done in accordance to the
			// slot duration and expected target block time, for safely
			// resisting network delays of maximum two seconds.
			// <https://research.web3.foundation/en/latest/polkadot/BABE/Babe/#6-practical-results>
			babe_primitives::BabeGenesisConfiguration {
				slot_duration: Babe::slot_duration(),
				epoch_length: EpochDuration::get(),
				c: PRIMARY_PROBABILITY,
				genesis_authorities: Babe::authorities(),
				randomness: Babe::randomness(),
				allowed_slots: babe_primitives::AllowedSlots::PrimaryAndSecondaryPlainSlots,
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
			AuthorityDiscovery::authorities()
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
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, RuntimeString> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, add_benchmark, TrackedStorageKey};
			// Trying to add benchmarks directly to the Session Pallet caused cyclic dependency issues.
			// To get around that, we separated the Session benchmarks into its own crate, which is why
			// we need these two lines below.
			use pallet_session_benchmarking::Module as SessionBench;
			use pallet_offences_benchmarking::Module as OffencesBench;
			use frame_system_benchmarking::Module as SystemBench;

			impl pallet_session_benchmarking::Config for Runtime {}
			impl pallet_offences_benchmarking::Config for Runtime {}
			impl frame_system_benchmarking::Config for Runtime {}

			let whitelist: Vec<TrackedStorageKey> = vec![
				// Block Number
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac").to_vec().into(),
				// Total Issuance
				hex_literal::hex!("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80").to_vec().into(),
				// Execution Phase
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a").to_vec().into(),
				// Event Count
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850").to_vec().into(),
				// System Events
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7").to_vec().into(),
				// Treasury Account
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7b99d880ec681799c0cf30e8886371da95ecffd7b6c0f78751baa9d281e0bfa3a6d6f646c70792f74727372790000000000000000000000000000000000000000").to_vec().into(),
			];

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			// Polkadot
			add_benchmark!(params, batches, runtime_common::claims, Claims);
			// Substrate
			add_benchmark!(params, batches, pallet_balances, Balances);
			add_benchmark!(params, batches, pallet_bounties, Bounties);
			add_benchmark!(params, batches, pallet_collective, Council);
			add_benchmark!(params, batches, pallet_democracy, Democracy);
			add_benchmark!(params, batches, pallet_elections_phragmen, ElectionsPhragmen);
			add_benchmark!(params, batches, pallet_identity, Identity);
			add_benchmark!(params, batches, pallet_im_online, ImOnline);
			add_benchmark!(params, batches, pallet_indices, Indices);
			add_benchmark!(params, batches, pallet_multisig, Multisig);
			add_benchmark!(params, batches, pallet_offences, OffencesBench::<Runtime>);
			add_benchmark!(params, batches, pallet_proxy, Proxy);
			add_benchmark!(params, batches, pallet_scheduler, Scheduler);
			add_benchmark!(params, batches, pallet_session, SessionBench::<Runtime>);
			add_benchmark!(params, batches, pallet_staking, Staking);
			add_benchmark!(params, batches, frame_system, SystemBench::<Runtime>);
			add_benchmark!(params, batches, pallet_timestamp, Timestamp);
			add_benchmark!(params, batches, pallet_tips, Tips);
			add_benchmark!(params, batches, pallet_treasury, Treasury);
			add_benchmark!(params, batches, pallet_utility, Utility);
			add_benchmark!(params, batches, pallet_vesting, Vesting);

			if batches.is_empty() { return Err("Benchmark not found for this pallet.".into()) }
			Ok(batches)
		}
	}
}

#[cfg(test)]
mod test_fees {
	use super::*;
	use frame_support::weights::WeightToFeePolynomial;
	use frame_support::storage::StorageValue;
	use sp_runtime::FixedPointNumber;
	use frame_support::weights::GetDispatchInfo;
	use parity_scale_codec::Encode;
	use pallet_transaction_payment::Multiplier;
	use separator::Separatable;


	#[test]
	#[ignore]
	fn block_cost() {
		let max_block_weight = BlockWeights::get().max_block;
		let raw_fee = WeightToFee::calc(&max_block_weight);

		println!(
			"Full Block weight == {} // WeightToFee(full_block) == {} plank",
			max_block_weight,
			raw_fee.separated_string(),
		);
	}

	#[test]
	#[ignore]
	fn transfer_cost_min_multiplier() {
		let min_multiplier = runtime_common::MinimumMultiplier::get();
		let call = <pallet_balances::Call<Runtime>>::transfer_keep_alive(Default::default(), Default::default());
		let info = call.get_dispatch_info();
		// convert to outer call.
		let call = Call::Balances(call);
		let len = call.using_encoded(|e| e.len()) as u32;

		let mut ext = sp_io::TestExternalities::new_empty();
		ext.execute_with(|| {
			pallet_transaction_payment::NextFeeMultiplier::put(min_multiplier);
			let fee = TransactionPayment::compute_fee(len, &info, 0);
			println!(
				"weight = {:?} // multiplier = {:?} // full transfer fee = {:?}",
				info.weight.separated_string(),
				pallet_transaction_payment::NextFeeMultiplier::get(),
				fee.separated_string(),
			);
		});

		ext.execute_with(|| {
			let mul = Multiplier::saturating_from_rational(1, 1000_000_000u128);
			pallet_transaction_payment::NextFeeMultiplier::put(mul);
			let fee = TransactionPayment::compute_fee(len, &info, 0);
			println!(
				"weight = {:?} // multiplier = {:?} // full transfer fee = {:?}",
				info.weight.separated_string(),
				pallet_transaction_payment::NextFeeMultiplier::get(),
				fee.separated_string(),
			);
		});

		ext.execute_with(|| {
			let mul = Multiplier::saturating_from_rational(1, 1u128);
			pallet_transaction_payment::NextFeeMultiplier::put(mul);
			let fee = TransactionPayment::compute_fee(len, &info, 0);
			println!(
				"weight = {:?} // multiplier = {:?} // full transfer fee = {:?}",
				info.weight.separated_string(),
				pallet_transaction_payment::NextFeeMultiplier::get(),
				fee.separated_string(),
			);
		});
	}
}
