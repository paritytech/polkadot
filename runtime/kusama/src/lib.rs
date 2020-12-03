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
	PersistedValidationData, Signature, ValidationCode, ValidationData, ValidatorId, ValidatorIndex,
	InboundDownwardMessage, InboundHrmpMessage, SessionInfo,
};
use runtime_common::{
	claims, SlowAdjustingFeeUpdate, CurrencyToVote,
	impls::DealWithFees,
	BlockHashCount, MaximumBlockWeight, AvailableBlockRatio,
	MaximumBlockLength, BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight,
	MaximumExtrinsicWeight, ParachainSessionKeyPlaceholder,
};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys, ModuleId,
	ApplyExtrinsicResult, KeyTypeId, Percent, Permill, Perbill,
	transaction_validity::{TransactionValidity, TransactionSource, TransactionPriority},
	curve::PiecewiseLinear,
	traits::{
		BlakeTwo256, Block as BlockT, OpaqueKeys, ConvertInto, IdentityLookup,
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
use pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo;
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
	spec_version: 2027,
	impl_version: 0,
	#[cfg(not(feature = "disable-runtime-api"))]
	apis: RUNTIME_API_VERSIONS,
	#[cfg(feature = "disable-runtime-api")]
	apis: version::create_apis_vec![[]],
	transaction_version: 3,
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
}

impl frame_system::Config for Runtime {
	type BaseCallFilter = BaseFilter;
	type Origin = Origin;
	type Call = Call;
	type Index = Nonce;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = generic::Header<BlockNumber, BlakeTwo256>;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type MaximumBlockWeight = MaximumBlockWeight;
	type DbWeight = RocksDbWeight;
	type BlockExecutionWeight = BlockExecutionWeight;
	type ExtrinsicBaseWeight = ExtrinsicBaseWeight;
	type MaximumExtrinsicWeight = MaximumExtrinsicWeight;
	type MaximumBlockLength = MaximumBlockLength;
	type AvailableBlockRatio = AvailableBlockRatio;
	type Version = Version;
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxScheduledPerBlock: u32 = 50;
}

impl pallet_scheduler::Config for Runtime {
	type Event = Event;
	type Origin = Origin;
	type PalletsOrigin = OriginCaller;
	type Call = Call;
	type MaximumWeight = MaximumBlockWeight;
	type ScheduleOrigin = EnsureRoot<AccountId>;
	type MaxScheduledPerBlock = MaxScheduledPerBlock;
	type WeightInfo = weights::pallet_scheduler::WeightInfo<Runtime>;
}

parameter_types! {
	pub const EpochDuration: u64 = EPOCH_DURATION_IN_BLOCKS as u64;
	pub const ExpectedBlockTime: Moment = MILLISECS_PER_BLOCK;
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
		pallet_babe::EquivocationHandler<Self::KeyOwnerIdentification, Offences>;

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
	pub struct SessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub im_online: ImOnline,
		pub parachain_validator: ParachainSessionKeyPlaceholder<Runtime>,
		pub authority_discovery: AuthorityDiscovery,
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
	pub OffchainSolutionWeightLimit: Weight = MaximumExtrinsicWeight::get()
		.saturating_sub(BlockExecutionWeight::get())
		.saturating_sub(ExtrinsicBaseWeight::get());
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
	pub const VotingBond: Balance = 5 * CENTS;
	/// Daily council elections.
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
	type VotingBond = VotingBond;
	type LoserCandidate = Treasury;
	type BadReport = Treasury;
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
	type Tippers = ElectionsPhragmen;
	type TipCountdown = TipCountdown;
	type TipFindersFee = TipFindersFee;
	type TipReportDepositBase = TipReportDepositBase;
	type DataDepositPerByte = DataDepositPerByte;
	type Event = Event;
	type OnSlash = Treasury;
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type SpendPeriod = SpendPeriod;
	type Burn = Burn;
	type BountyDepositBase = BountyDepositBase;
	type BountyDepositPayoutDelay = BountyDepositPayoutDelay;
	type BountyUpdatePeriod = BountyUpdatePeriod;
	type MaximumReasonLength = MaximumReasonLength;
	type BountyCuratorDeposit = BountyCuratorDeposit;
	type BountyValueMinimum = BountyValueMinimum;
	type BurnDestination = Society;
	type WeightInfo = weights::pallet_treasury::WeightInfo<Runtime>;
}

parameter_types! {
	pub OffencesWeightSoftLimit: Weight = Perbill::from_percent(60) * MaximumBlockWeight::get();
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

	type HandleEquivocation = pallet_grandpa::EquivocationHandler<Self::KeyOwnerIdentification, Offences>;

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
		Some((call, (account, signature, extra)))
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

pub struct FixCouncilHistoricalVotes;
impl frame_support::traits::OnRuntimeUpgrade for FixCouncilHistoricalVotes {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		use frame_support::traits::ReservableCurrency;
		use sp_runtime::traits::Zero;
		let mut failure: Balance = 0;
		// https://github.com/paritytech/polkadot/pull/1252/files#diff-cba4e599a9fdd88fe8d33b5ed913958d63f844186b53c5cbe9bc73a2e2944857R22

		// https://polkascan.io/kusama/runtime-module/2007-electionsphragmen
		let old_bond = 50_000_000_000;
		// https://polkascan.io/kusama/runtime-module/2008-electionsphragmen
		let current_bond = 8_333_333_330;
		let to_unreserve = old_bond - current_bond;   // 41666666670

		// source of accounts: https://github.com/paritytech/substrate/issues/7223
		vec![
			[52u8, 227, 117, 17, 229, 245, 8, 66, 43, 10, 142, 216, 196, 102, 119, 154, 34, 41, 53, 183, 37, 186, 250, 70, 247, 129, 207, 56, 2, 96, 181, 69],
			[87, 71, 87, 4, 112, 230, 183, 229, 153, 158, 195, 253, 122, 165, 32, 37, 212, 105, 167, 124, 20, 165, 83, 106, 177, 214, 223, 18, 146, 184, 186, 42],
			[74, 223, 81, 164, 123, 114, 121, 83, 102, 213, 34, 133, 227, 41, 34, 156, 131, 110, 167, 187, 254, 19, 157, 190, 143, 160, 112, 12, 79, 134, 252, 86],
			[98, 135, 195, 120, 192, 49, 156, 220, 141, 79, 176, 216, 27, 229, 80, 37, 72, 104, 114, 242, 254, 188, 218, 156, 66, 143, 164, 131, 182, 181, 43, 27],
			[22, 106, 142, 133, 251, 42, 232, 228, 187, 104, 21, 64, 122, 178, 225, 117, 115, 5, 10, 8, 14, 27, 171, 197, 2, 34, 100, 254, 249, 233, 111, 94],
			[230, 17, 194, 236, 237, 27, 86, 17, 131, 248, 143, 174, 208, 221, 125, 136, 213, 250, 253, 241, 111, 88, 64, 198, 62, 195, 109, 140, 49, 19, 111, 97],
			[45, 100, 142, 202, 87, 103, 177, 184, 106, 165, 70, 32, 79, 239, 241, 127, 98, 45, 74, 19, 53, 72, 54, 34, 95, 212, 237, 10, 49, 18, 118, 11],
			[78, 212, 66, 185, 0, 51, 101, 94, 134, 29, 31, 236, 213, 26, 156, 115, 199, 195, 117, 27, 34, 125, 115, 175, 37, 139, 73, 23, 110, 16, 121, 19],
			[198, 17, 209, 81, 89, 27, 253, 242, 89, 118, 43, 153, 183, 128, 97, 97, 123, 89, 210, 171, 23, 66, 63, 32, 239, 233, 142, 222, 32, 184, 217, 120],
			[48, 89, 157, 186, 80, 181, 243, 186, 11, 54, 248, 86, 167, 97, 235, 60, 10, 238, 97, 232, 48, 212, 190, 180, 72, 239, 148, 182, 173, 146, 190, 57],
			[178, 75, 65, 145, 80, 177, 162, 44, 37, 159, 216, 50, 26, 48, 88, 234, 131, 168, 17, 141, 41, 235, 11, 196, 110, 0, 86, 230, 249, 136, 148, 39],
			[0, 90, 67, 60, 142, 21, 28, 129, 174, 148, 133, 68, 244, 203, 7, 98, 43, 24, 168, 67, 4, 128, 222, 111, 198, 225, 163, 139, 196, 111, 156, 39],
			[80, 10, 128, 247, 239, 148, 61, 30, 111, 173, 141, 133, 33, 169, 238, 221, 44, 22, 26, 149, 224, 64, 133, 242, 123, 198, 162, 35, 123, 47, 17, 57],
			[228, 248, 227, 202, 10, 103, 4, 160, 7, 148, 69, 176, 153, 221, 192, 80, 193, 253, 39, 48, 70, 249, 58, 115, 4, 15, 66, 115, 105, 58, 184, 61],
			[146, 142, 243, 123, 168, 13, 37, 253, 223, 148, 61, 229, 35, 244, 110, 88, 140, 135, 188, 134, 227, 131, 24, 149, 242, 125, 169, 157, 38, 154, 160, 18],
			[12, 55, 156, 202, 114, 167, 250, 113, 52, 125, 148, 219, 103, 69, 77, 226, 216, 92, 20, 234, 202, 146, 140, 75, 76, 99, 153, 156, 27, 168, 164, 48],
			[94, 105, 67, 219, 185, 200, 207, 213, 51, 119, 166, 115, 7, 41, 14, 250, 193, 175, 244, 170, 35, 242, 134, 43, 216, 100, 10, 243, 117, 111, 121, 44],
			[176, 235, 16, 242, 219, 110, 35, 128, 177, 12, 46, 128, 32, 93, 131, 158, 3, 181, 150, 226, 40, 253, 141, 242, 188, 117, 191, 197, 150, 174, 171, 36],
			[188, 94, 5, 123, 119, 210, 246, 167, 145, 84, 105, 228, 217, 124, 68, 191, 165, 211, 135, 133, 201, 241, 211, 8, 146, 250, 25, 231, 234, 206, 57, 57],
			[190, 109, 228, 0, 24, 21, 61, 124, 206, 0, 67, 246, 131, 206, 237, 153, 207, 59, 48, 135, 152, 89, 96, 151, 169, 64, 107, 186, 201, 145, 144, 21],
			[168, 176, 158, 34, 73, 77, 195, 235, 190, 198, 231, 174, 81, 174, 202, 99, 219, 183, 220, 4, 216, 95, 64, 254, 135, 161, 130, 228, 157, 18, 205, 122],
			[58, 175, 247, 7, 11, 38, 34, 147, 124, 193, 15, 99, 218, 12, 92, 232, 75, 72, 123, 210, 200, 62, 174, 59, 183, 5, 78, 112, 137, 169, 221, 5],
			[38, 132, 41, 39, 201, 138, 80, 171, 29, 67, 154, 180, 95, 33, 197, 190, 182, 151, 5, 86, 225, 253, 123, 82, 223, 68, 151, 126, 67, 68, 177, 72],
			[160, 50, 214, 174, 242, 243, 162, 74, 49, 196, 28, 253, 251, 33, 243, 155, 163, 253, 207, 201, 237, 31, 56, 185, 22, 125, 172, 178, 228, 61, 116, 124],
			[94, 237, 179, 116, 143, 73, 1, 160, 48, 111, 172, 136, 170, 109, 127, 28, 131, 61, 146, 143, 219, 236, 250, 236, 67, 247, 90, 172, 31, 95, 125, 122],
			[136, 143, 102, 104, 40, 232, 50, 138, 51, 100, 122, 71, 188, 151, 87, 74, 106, 86, 113, 129, 146, 112, 204, 1, 230, 108, 113, 57, 161, 166, 145, 26],
			[41, 76, 90, 193, 202, 37, 94, 199, 50, 139, 43, 253, 174, 91, 152, 164, 163, 181, 13, 201, 149, 100, 7, 183, 161, 145, 13, 143, 215, 229, 129, 232],
			[16, 252, 67, 246, 61, 252, 235, 195, 3, 194, 11, 182, 243, 47, 162, 8, 197, 85, 240, 183, 52, 85, 172, 246, 161, 197, 65, 200, 79, 219, 177, 104],
			[160, 87, 16, 231, 9, 55, 108, 216, 216, 28, 145, 235, 37, 92, 96, 16, 52, 194, 45, 134, 150, 78, 181, 46, 183, 229, 201, 35, 45, 19, 176, 94],
			[134, 135, 73, 95, 235, 234, 33, 222, 68, 159, 242, 115, 129, 249, 48, 141, 166, 241, 92, 229, 217, 211, 20, 98, 97, 39, 93, 236, 24, 205, 86, 111],
			[251, 174, 188, 92, 115, 39, 20, 75, 229, 29, 243, 91, 181, 15, 248, 97, 44, 140, 154, 215, 63, 199, 182, 11, 67, 130, 185, 121, 86, 61, 226, 15],
			[190, 224, 239, 104, 232, 185, 30, 26, 131, 177, 69, 35, 42, 159, 216, 68, 170, 200, 161, 101, 95, 61, 114, 21, 61, 99, 221, 132, 47, 71, 6, 100],
			[132, 237, 28, 134, 11, 165, 89, 21, 143, 203, 78, 152, 122, 33, 213, 210, 155, 117, 79, 248, 141, 180, 215, 75, 125, 214, 64, 79, 188, 233, 114, 22],
			[203, 124, 199, 178, 246, 36, 201, 44, 111, 173, 142, 231, 116, 88, 163, 92, 122, 202, 173, 226, 176, 62, 95, 6, 52, 80, 156, 239, 29, 183, 206, 9],
			[178, 38, 5, 179, 106, 208, 161, 253, 17, 62, 16, 224, 250, 91, 72, 135, 21, 160, 113, 252, 152, 33, 173, 20, 68, 167, 33, 102, 67, 28, 30, 21],
			[0, 85, 93, 35, 172, 249, 206, 242, 240, 251, 36, 168, 255, 45, 70, 79, 228, 161, 147, 137, 98, 46, 36, 1, 38, 15, 73, 36, 114, 171, 123, 70],
			[198, 88, 98, 42, 56, 161, 58, 36, 180, 89, 254, 109, 16, 255, 214, 120, 192, 204, 248, 245, 145, 124, 72, 217, 139, 9, 182, 116, 98, 86, 9, 26],
			[178, 219, 195, 92, 207, 8, 98, 148, 160, 210, 78, 16, 145, 208, 140, 163, 181, 194, 164, 135, 7, 28, 79, 181, 64, 112, 230, 102, 204, 153, 224, 45],
			[118, 253, 161, 198, 240, 206, 6, 239, 41, 107, 105, 123, 178, 23, 249, 142, 69, 146, 242, 95, 20, 113, 228, 97, 146, 148, 115, 55, 146, 48, 147, 173],
			[171, 42, 226, 38, 198, 62, 131, 93, 136, 64, 239, 182, 111, 170, 191, 132, 59, 203, 110, 239, 70, 42, 12, 117, 248, 87, 48, 58, 24, 193, 214, 207],
			[226, 156, 174, 201, 243, 176, 175, 214, 64, 12, 186, 43, 40, 42, 230, 20, 41, 71, 218, 167, 131, 80, 249, 155, 42, 116, 123, 52, 44, 42, 25, 64],
			[38, 233, 51, 113, 227, 226, 183, 195, 139, 229, 42, 201, 30, 142, 166, 33, 165, 173, 117, 24, 213, 88, 15, 167, 179, 109, 37, 11, 158, 211, 87, 26],
			[28, 82, 239, 62, 195, 223, 46, 66, 201, 184, 90, 253, 224, 20, 86, 231, 70, 19, 20, 166, 143, 22, 94, 166, 11, 34, 2, 175, 87, 13, 17, 20],
			[6, 121, 215, 46, 243, 76, 78, 115, 130, 220, 90, 195, 3, 135, 100, 66, 46, 201, 243, 74, 103, 244, 214, 70, 253, 30, 228, 245, 93, 182, 92, 27],
			[56, 242, 67, 184, 105, 96, 247, 25, 150, 176, 97, 251, 46, 223, 29, 42, 114, 79, 82, 223, 42, 165, 104, 95, 225, 132, 222, 222, 236, 237, 180, 70],
			[206, 163, 218, 190, 82, 178, 166, 101, 177, 225, 155, 248, 198, 145, 58, 93, 84, 224, 109, 100, 19, 202, 61, 219, 236, 143, 154, 34, 65, 94, 196, 119],
			[32, 51, 169, 66, 133, 238, 5, 16, 36, 249, 231, 26, 132, 203, 51, 48, 85, 127, 124, 4, 154, 5, 45, 96, 136, 44, 186, 14, 212, 82, 209, 45],
			[136, 87, 179, 203, 183, 159, 117, 238, 119, 98, 216, 164, 49, 132, 57, 146, 127, 210, 181, 22, 67, 156, 89, 113, 52, 195, 208, 159, 224, 227, 241, 3],
			[58, 69, 248, 95, 254, 189, 177, 143, 25, 199, 92, 139, 237, 97, 234, 17, 219, 250, 40, 132, 41, 202, 235, 238, 203, 35, 33, 26, 73, 237, 165, 32],
			[146, 24, 163, 171, 202, 106, 170, 124, 218, 48, 242, 73, 62, 87, 229, 38, 27, 6, 15, 95, 57, 47, 45, 76, 221, 154, 171, 55, 19, 227, 61, 60],
			[60, 58, 195, 101, 58, 75, 249, 167, 40, 117, 131, 147, 187, 201, 189, 197, 202, 49, 226, 154, 237, 70, 161, 88, 95, 211, 212, 145, 2, 87, 200, 33],
			[230, 153, 129, 0, 226, 30, 98, 227, 216, 119, 32, 200, 72, 8, 114, 41, 148, 250, 98, 95, 100, 23, 108, 158, 149, 236, 85, 106, 118, 13, 64, 78],
			[208, 159, 158, 0, 216, 253, 73, 87, 0, 248, 236, 76, 249, 90, 162, 232, 39, 227, 251, 183, 239, 0, 130, 254, 46, 202, 75, 146, 104, 48, 250, 29],
			[206, 65, 0, 132, 231, 167, 48, 145, 37, 141, 211, 98, 59, 98, 217, 50, 157, 101, 135, 114, 63, 194, 96, 210, 142, 85, 21, 144, 133, 63, 93, 88],
			[58, 34, 87, 220, 204, 157, 71, 5, 126, 215, 168, 184, 84, 75, 160, 45, 84, 172, 6, 243, 13, 119, 230, 88, 140, 30, 21, 137, 150, 229, 20, 38],
			[202, 91, 193, 145, 93, 167, 74, 186, 58, 173, 215, 206, 123, 128, 144, 69, 213, 235, 91, 115, 85, 146, 89, 117, 95, 220, 216, 90, 64, 165, 220, 110],
			[10, 58, 158, 3, 226, 253, 136, 14, 137, 63, 60, 210, 253, 3, 181, 124, 125, 40, 29, 43, 70, 105, 185, 59, 16, 42, 148, 5, 43, 227, 101, 98],
			[172, 150, 113, 140, 115, 71, 210, 56, 57, 84, 225, 178, 82, 233, 29, 155, 84, 156, 238, 44, 60, 146, 176, 166, 170, 54, 96, 170, 124, 201, 81, 56],
			[158, 190, 208, 112, 142, 212, 167, 220, 247, 24, 86, 187, 83, 134, 53, 201, 255, 190, 70, 99, 40, 99, 7, 223, 197, 166, 14, 154, 188, 223, 70, 30],
			[60, 67, 92, 98, 149, 98, 142, 28, 126, 136, 184, 249, 235, 75, 188, 61, 96, 166, 59, 25, 140, 13, 201, 175, 192, 130, 4, 170, 74, 190, 195, 113],
			[78, 203, 3, 76, 75, 78, 165, 166, 103, 0, 12, 191, 228, 137, 234, 15, 122, 162, 12, 197, 222, 180, 111, 152, 25, 187, 100, 17, 157, 252, 83, 39],
			[146, 250, 178, 111, 64, 184, 149, 164, 242, 68, 16, 85, 67, 135, 47, 22, 85, 142, 224, 194, 245, 114, 165, 219, 48, 131, 56, 230, 241, 205, 118, 35],
			[111, 136, 30, 180, 158, 175, 45, 159, 88, 34, 172, 160, 141, 149, 18, 237, 72, 43, 243, 95, 36, 70, 169, 253, 20, 102, 134, 46, 122, 117, 94, 40],
			[230, 224, 55, 10, 146, 36, 6, 46, 185, 8, 5, 58, 133, 127, 124, 142, 115, 39, 215, 94, 175, 55, 41, 148, 133, 70, 80, 119, 188, 168, 103, 26],
			[88, 134, 227, 88, 24, 157, 191, 87, 39, 23, 227, 3, 155, 129, 197, 229, 132, 243, 115, 46, 114, 152, 182, 251, 24, 162, 203, 14, 223, 70, 110, 18],
			[78, 192, 56, 30, 68, 39, 237, 101, 103, 247, 165, 195, 40, 40, 140, 237, 54, 195, 59, 236, 234, 110, 206, 205, 129, 69, 0, 31, 66, 48, 172, 27],
			[188, 110, 18, 215, 171, 112, 171, 234, 76, 8, 219, 112, 85, 232, 79, 22, 186, 184, 23, 181, 251, 53, 144, 136, 173, 81, 144, 66, 45, 249, 221, 29],
			[184, 134, 3, 172, 197, 123, 71, 84, 219, 125, 44, 26, 224, 165, 217, 103, 32, 108, 191, 22, 216, 108, 41, 133, 56, 89, 83, 174, 178, 5, 143, 5],
			[10, 216, 180, 249, 77, 200, 230, 34, 158, 44, 68, 141, 153, 80, 148, 205, 193, 189, 53, 109, 193, 76, 97, 85, 70, 122, 192, 126, 222, 24, 184, 114],
			[26, 170, 217, 19, 57, 86, 181, 16, 1, 80, 222, 130, 169, 29, 138, 87, 109, 207, 182, 63, 199, 221, 13, 83, 54, 8, 57, 131, 149, 198, 208, 83],
			[96, 138, 24, 198, 63, 184, 175, 138, 213, 226, 226, 154, 248, 15, 23, 237, 238, 81, 195, 43, 137, 19, 196, 103, 238, 168, 38, 237, 103, 102, 37, 40],
			[52, 128, 169, 39, 185, 38, 19, 53, 116, 172, 54, 108, 87, 60, 188, 116, 37, 164, 126, 195, 94, 206, 39, 89, 153, 179, 209, 240, 131, 82, 156, 46],
			[246, 4, 145, 84, 210, 56, 187, 133, 217, 118, 194, 157, 220, 55, 43, 88, 228, 254, 223, 5, 126, 65, 104, 125, 12, 250, 57, 241, 71, 113, 171, 83],
			[86, 173, 152, 172, 190, 131, 221, 21, 171, 209, 16, 17, 30, 220, 112, 220, 192, 162, 19, 36, 91, 45, 44, 192, 169, 65, 10, 9, 51, 57, 255, 70],
			[64, 123, 211, 149, 104, 201, 8, 6, 47, 202, 49, 232, 8, 152, 189, 202, 190, 237, 160, 117, 1, 51, 131, 240, 249, 166, 158, 208, 126, 177, 38, 38],
			[2, 57, 183, 234, 172, 195, 234, 64, 151, 134, 240, 51, 106, 137, 118, 7, 86, 35, 172, 239, 49, 159, 197, 119, 124, 118, 3, 61, 213, 133, 184, 64],
			[96, 254, 164, 33, 61, 85, 200, 104, 191, 200, 140, 122, 127, 80, 64, 175, 89, 63, 213, 255, 88, 154, 127, 26, 93, 114, 70, 81, 223, 37, 5, 95],
			[72, 35, 54, 126, 94, 99, 159, 33, 213, 118, 137, 168, 157, 235, 63, 72, 148, 114, 187, 16, 4, 122, 103, 117, 103, 88, 162, 148, 218, 167, 159, 21],
			[232, 206, 1, 108, 146, 138, 182, 169, 95, 61, 218, 93, 127, 149, 24, 50, 55, 80, 176, 2, 18, 205, 131, 111, 249, 163, 241, 242, 126, 178, 193, 33],
			[248, 254, 82, 84, 191, 224, 104, 1, 129, 7, 9, 121, 239, 231, 44, 94, 176, 153, 4, 59, 48, 7, 79, 48, 221, 12, 21, 168, 74, 188, 68, 92],
			[2, 156, 106, 91, 42, 221, 67, 178, 36, 110, 31, 47, 8, 233, 169, 131, 255, 102, 80, 228, 186, 141, 9, 32, 35, 145, 198, 162, 141, 60, 223, 54],
			[0, 95, 174, 86, 79, 8, 222, 91, 181, 144, 141, 255, 246, 191, 240, 249, 80, 123, 116, 75, 33, 215, 1, 125, 71, 138, 167, 239, 92, 135, 249, 124],
			[4, 198, 135, 31, 33, 23, 62, 34, 187, 204, 153, 2, 161, 186, 65, 165, 19, 204, 95, 255, 121, 124, 148, 138, 54, 146, 124, 239, 112, 20, 140, 48],
			[146, 46, 66, 112, 210, 142, 32, 160, 129, 86, 195, 218, 234, 150, 130, 77, 79, 69, 30, 232, 224, 12, 77, 254, 7, 81, 203, 63, 65, 228, 187, 74],
			[52, 234, 22, 159, 11, 191, 106, 184, 97, 55, 123, 62, 156, 195, 78, 82, 255, 163, 241, 103, 79, 136, 123, 113, 177, 75, 50, 64, 66, 33, 177, 53],
			[10, 122, 197, 190, 105, 168, 36, 63, 136, 128, 213, 253, 1, 91, 46, 143, 143, 48, 206, 108, 113, 98, 248, 188, 181, 173, 26, 31, 164, 36, 109, 50],
			[10, 91, 84, 200, 115, 95, 146, 200, 152, 137, 149, 161, 91, 207, 61, 17, 192, 46, 232, 218, 103, 99, 52, 168, 162, 144, 252, 116, 63, 99, 73, 40],
			[36, 123, 240, 229, 60, 125, 242, 213, 41, 87, 26, 15, 48, 180, 88, 19, 205, 151, 252, 208, 8, 248, 210, 15, 180, 43, 68, 160, 205, 95, 28, 119],
			[142, 57, 249, 121, 182, 35, 220, 93, 141, 234, 130, 249, 187, 90, 126, 152, 100, 181, 181, 61, 85, 2, 201, 139, 200, 140, 14, 115, 199, 49, 192, 14],
			[132, 70, 235, 131, 233, 186, 168, 74, 114, 31, 172, 138, 150, 168, 7, 117, 176, 86, 48, 31, 223, 126, 113, 95, 57, 141, 125, 203, 37, 249, 174, 114],
			[164, 213, 85, 73, 205, 119, 18, 200, 239, 149, 51, 108, 167, 171, 251, 28, 232, 84, 51, 51, 30, 72, 84, 172, 255, 170, 232, 72, 135, 12, 105, 6],
			[214, 194, 236, 50, 109, 31, 114, 151, 96, 221, 23, 131, 234, 33, 109, 164, 43, 212, 147, 65, 13, 192, 151, 171, 47, 139, 85, 207, 241, 109, 226, 37],
			[25, 148, 223, 91, 240, 244, 67, 66, 177, 113, 155, 251, 177, 86, 18, 134, 189, 129, 182, 216, 79, 87, 127, 85, 239, 69, 254, 122, 214, 245, 14, 74],
			[68, 16, 115, 21, 34, 226, 104, 3, 184, 230, 235, 110, 84, 103, 215, 122, 170, 5, 6, 132, 185, 87, 34, 187, 166, 96, 136, 44, 144, 169, 208, 21],
			[92, 143, 180, 46, 128, 189, 71, 207, 86, 229, 246, 37, 92, 23, 88, 25, 163, 73, 234, 107, 147, 239, 18, 125, 118, 57, 132, 179, 253, 113, 79, 49],
			[152, 97, 132, 18, 9, 74, 115, 6, 101, 205, 185, 117, 139, 71, 65, 181, 84, 53, 3, 174, 8, 178, 181, 247, 154, 70, 3, 147, 89, 138, 183, 54],
			[117, 159, 129, 181, 10, 57, 31, 216, 133, 197, 227, 207, 216, 106, 49, 242, 18, 70, 125, 101, 88, 44, 149, 1, 10, 72, 187, 48, 210, 126, 209, 231],
			[230, 213, 178, 217, 236, 22, 235, 17, 122, 106, 200, 208, 125, 215, 17, 51, 126, 87, 75, 194, 187, 122, 246, 10, 57, 213, 62, 197, 108, 139, 115, 89],
			[56, 85, 62, 17, 98, 50, 252, 144, 165, 195, 142, 14, 85, 228, 46, 97, 195, 219, 204, 67, 197, 178, 64, 234, 124, 62, 50, 179, 125, 103, 201, 81],
			[184, 253, 244, 203, 162, 173, 242, 65, 221, 223, 194, 0, 136, 194, 60, 114, 56, 128, 185, 125, 197, 65, 244, 137, 5, 217, 158, 177, 186, 14, 92, 39],
			[160, 76, 27, 164, 78, 128, 105, 139, 142, 143, 248, 18, 107, 138, 77, 120, 70, 196, 126, 223, 48, 55, 194, 172, 131, 28, 239, 131, 36, 2, 89, 28],
			[186, 25, 173, 248, 171, 133, 40, 201, 245, 48, 88, 180, 148, 182, 21, 77, 222, 15, 173, 254, 43, 222, 179, 169, 185, 200, 119, 97, 205, 203, 180, 65],
			[12, 76, 85, 245, 143, 131, 207, 130, 43, 102, 255, 202, 240, 87, 249, 239, 185, 252, 101, 71, 87, 85, 3, 232, 17, 88, 172, 202, 13, 145, 101, 27],
			[113, 153, 171, 173, 152, 127, 178, 8, 186, 128, 74, 4, 122, 115, 23, 37, 195, 7, 45, 117, 37, 238, 162, 188, 223, 217, 127, 168, 193, 76, 138, 119],
			[12, 206, 158, 33, 12, 71, 63, 209, 242, 1, 120, 254, 136, 156, 23, 137, 86, 234, 28, 243, 37, 197, 75, 26, 67, 154, 136, 188, 98, 254, 120, 81],
			[134, 213, 134, 159, 7, 115, 242, 48, 151, 43, 141, 107, 62, 252, 233, 210, 189, 93, 155, 169, 218, 86, 103, 181, 166, 136, 166, 251, 103, 252, 201, 36],
			[156, 152, 138, 156, 80, 10, 196, 114, 228, 177, 236, 190, 171, 59, 16, 81, 77, 203, 139, 205, 80, 8, 183, 26, 32, 234, 161, 191, 40, 29, 168, 15],
			[96, 132, 24, 217, 54, 66, 26, 130, 142, 118, 240, 102, 152, 105, 47, 47, 66, 53, 132, 35, 4, 42, 239, 229, 119, 171, 238, 44, 33, 41, 228, 187],
			[38, 43, 59, 107, 223, 253, 235, 155, 48, 76, 96, 233, 143, 87, 248, 107, 239, 214, 130, 34, 67, 94, 60, 243, 23, 172, 32, 79, 79, 55, 112, 78],
			[246, 178, 29, 98, 72, 50, 9, 75, 3, 170, 103, 46, 1, 100, 98, 160, 32, 226, 23, 204, 103, 177, 67, 71, 133, 185, 145, 20, 162, 180, 250, 90],
			[138, 152, 73, 84, 229, 126, 123, 240, 75, 163, 140, 241, 166, 30, 215, 71, 131, 212, 202, 118, 116, 76, 63, 169, 246, 220, 10, 253, 85, 217, 23, 71],
			[38, 207, 39, 144, 245, 25, 234, 121, 233, 220, 11, 81, 64, 16, 219, 209, 75, 187, 207, 106, 139, 84, 32, 107, 108, 178, 68, 20, 3, 5, 236, 112],
			[64, 255, 129, 147, 44, 86, 190, 113, 168, 32, 124, 138, 153, 50, 141, 96, 165, 162, 176, 111, 212, 14, 208, 94, 196, 178, 214, 106, 235, 202, 255, 104],
			[44, 25, 247, 67, 149, 0, 166, 187, 208, 78, 125, 185, 236, 25, 139, 4, 89, 160, 4, 196, 128, 47, 39, 229, 0, 254, 77, 248, 122, 61, 227, 27],
			[174, 206, 85, 8, 225, 55, 152, 52, 175, 47, 168, 28, 167, 138, 137, 244, 103, 82, 129, 11, 37, 53, 123, 150, 243, 158, 203, 190, 18, 195, 200, 55],
			[190, 243, 241, 170, 113, 179, 43, 186, 119, 91, 56, 134, 185, 0, 162, 227, 251, 79, 65, 99, 213, 140, 27, 206, 10, 174, 207, 224, 181, 92, 27, 95],
			[218, 214, 230, 25, 76, 32, 165, 14, 194, 19, 56, 71, 77, 52, 110, 93, 38, 112, 237, 19, 172, 17, 68, 117, 145, 189, 5, 133, 201, 124, 200, 101],
			[146, 73, 247, 0, 26, 190, 182, 82, 240, 43, 224, 199, 223, 167, 173, 151, 130, 188, 113, 208, 86, 81, 255, 20, 235, 214, 89, 225, 229, 159, 130, 126],
			[204, 88, 161, 4, 79, 211, 105, 244, 82, 11, 187, 174, 226, 18, 241, 32, 61, 124, 179, 97, 27, 84, 80, 153, 243, 137, 134, 27, 145, 28, 2, 90],
			[178, 33, 243, 211, 58, 219, 171, 225, 105, 91, 109, 239, 143, 159, 179, 179, 10, 51, 201, 238, 226, 231, 176, 36, 52, 17, 82, 213, 253, 187, 226, 51],
			[172, 29, 45, 130, 196, 166, 155, 22, 195, 206, 158, 181, 208, 182, 243, 79, 148, 138, 52, 239, 230, 36, 136, 135, 154, 81, 75, 188, 131, 126, 14, 80],
			[126, 194, 148, 162, 173, 83, 41, 233, 36, 136, 220, 29, 232, 46, 77, 165, 208, 239, 112, 206, 133, 36, 44, 15, 93, 22, 174, 219, 36, 96, 0, 125],
			[182, 191, 157, 11, 214, 231, 26, 222, 121, 107, 197, 21, 181, 99, 44, 71, 187, 157, 143, 154, 229, 81, 95, 52, 45, 55, 23, 134, 255, 110, 90, 30],
			[162, 160, 236, 188, 172, 133, 147, 194, 200, 66, 108, 85, 218, 66, 110, 32, 41, 3, 162, 118, 183, 33, 255, 117, 139, 139, 110, 108, 2, 96, 52, 5],
			[218, 18, 91, 123, 235, 68, 15, 182, 161, 69, 168, 24, 157, 227, 50, 42, 108, 168, 226, 83, 193, 19, 39, 128, 139, 41, 198, 42, 232, 118, 176, 13],
			[218, 214, 145, 46, 29, 34, 180, 161, 82, 185, 48, 163, 42, 136, 88, 162, 4, 109, 16, 187, 21, 166, 51, 211, 124, 151, 142, 222, 173, 110, 119, 46],
			[94, 215, 163, 23, 159, 65, 29, 10, 174, 240, 104, 130, 69, 139, 87, 245, 27, 53, 80, 145, 184, 70, 187, 54, 96, 153, 66, 109, 80, 25, 162, 82],
			[104, 214, 130, 92, 100, 194, 124, 40, 175, 70, 14, 143, 173, 49, 59, 178, 254, 215, 90, 255, 89, 232, 223, 153, 179, 237, 202, 237, 236, 150, 216, 102],
			[166, 101, 158, 76, 63, 34, 194, 170, 151, 213, 74, 54, 227, 26, 181, 122, 97, 122, 246, 43, 212, 62, 198, 46, 213, 112, 119, 20, 146, 6, 146, 112],
			[20, 229, 93, 235, 203, 26, 151, 13, 177, 181, 31, 83, 86, 1, 8, 13, 18, 141, 245, 223, 242, 89, 63, 238, 30, 51, 105, 19, 157, 81, 192, 114],
			[44, 36, 100, 44, 239, 20, 231, 115, 21, 191, 70, 124, 0, 145, 124, 116, 154, 25, 195, 229, 166, 223, 112, 85, 72, 166, 122, 167, 173, 10, 209, 56],
			[142, 133, 30, 217, 146, 34, 143, 34, 104, 238, 140, 97, 79, 230, 7, 93, 56, 0, 6, 10, 225, 64, 152, 224, 48, 148, 19, 160, 168, 28, 68, 112],
			[84, 94, 128, 100, 248, 137, 138, 41, 212, 129, 30, 9, 178, 7, 207, 51, 2, 229, 206, 254, 241, 102, 21, 248, 88, 15, 205, 143, 166, 58, 98, 78],
			[218, 47, 127, 176, 63, 207, 248, 72, 142, 2, 155, 189, 98, 249, 82, 112, 244, 5, 195, 2, 137, 92, 194, 133, 100, 166, 158, 6, 144, 50, 230, 116],
			[42, 138, 54, 49, 198, 224, 120, 197, 217, 30, 242, 215, 114, 10, 252, 175, 64, 173, 186, 66, 90, 100, 138, 128, 130, 66, 13, 125, 7, 140, 71, 58],
			[156, 120, 182, 33, 219, 174, 128, 170, 103, 151, 162, 143, 117, 32, 89, 238, 241, 171, 215, 99, 218, 189, 163, 89, 85, 96, 160, 52, 143, 248, 46, 57],
			[232, 139, 71, 107, 182, 41, 146, 230, 64, 3, 205, 166, 216, 146, 173, 149, 225, 180, 93, 128, 227, 254, 240, 29, 10, 65, 25, 225, 235, 227, 163, 6],
			[121, 91, 9, 166, 254, 68, 24, 31, 178, 252, 33, 186, 252, 39, 149, 139, 185, 99, 188, 188, 73, 107, 169, 0, 92, 176, 6, 44, 242, 122, 240, 145],
			[18, 52, 99, 140, 43, 150, 145, 119, 163, 23, 246, 218, 246, 253, 90, 40, 104, 207, 68, 132, 217, 142, 158, 174, 83, 255, 207, 181, 178, 229, 182, 95],
			[64, 164, 10, 249, 72, 67, 69, 141, 42, 50, 223, 253, 168, 193, 19, 20, 60, 76, 38, 59, 104, 159, 178, 47, 235, 40, 23, 212, 75, 85, 116, 71],
			[90, 135, 58, 121, 143, 143, 110, 100, 254, 215, 107, 203, 160, 199, 182, 86, 86, 161, 81, 93, 144, 199, 51, 190, 175, 173, 102, 139, 228, 4, 116, 109],
			[62, 30, 163, 156, 6, 70, 240, 232, 22, 213, 96, 56, 232, 180, 57, 15, 60, 179, 203, 155, 153, 72, 62, 189, 153, 198, 5, 207, 52, 135, 38, 117],
			[44, 112, 144, 18, 248, 7, 175, 143, 195, 240, 210, 171, 176, 197, 28, 169, 168, 141, 78, 242, 77, 26, 9, 43, 248, 157, 172, 245, 206, 99, 234, 29],
			[28, 46, 116, 60, 92, 209, 172, 126, 74, 248, 247, 204, 141, 211, 239, 86, 31, 116, 155, 112, 215, 44, 170, 215, 182, 233, 212, 116, 28, 124, 47, 56],
			[250, 97, 238, 17, 124, 244, 135, 220, 57, 98, 15, 172, 108, 62, 133, 81, 17, 246, 132, 53, 130, 122, 28, 100, 104, 164, 91, 138, 183, 59, 122, 147],
			[58, 13, 33, 166, 234, 193, 159, 44, 11, 84, 97, 158, 123, 225, 71, 8, 234, 35, 71, 206, 84, 152, 118, 183, 248, 102, 3, 149, 189, 13, 86, 168],
			[210, 150, 179, 95, 208, 49, 151, 66, 83, 55, 119, 53, 143, 48, 183, 8, 170, 246, 179, 135, 9, 210, 90, 89, 246, 87, 110, 88, 22, 108, 209, 77],
			[78, 21, 80, 146, 0, 103, 4, 128, 134, 169, 243, 15, 121, 154, 23, 73, 80, 142, 34, 42, 209, 169, 217, 153, 245, 134, 230, 243, 231, 130, 201, 50],
			[172, 29, 49, 23, 191, 6, 255, 232, 145, 41, 74, 11, 29, 19, 218, 87, 78, 212, 129, 65, 9, 0, 161, 70, 196, 152, 211, 120, 21, 216, 97, 107],
			[171, 172, 81, 10, 126, 40, 213, 246, 82, 66, 253, 253, 50, 154, 112, 117, 40, 245, 162, 134, 93, 237, 142, 52, 41, 104, 176, 27, 1, 79, 238, 84],
			[122, 115, 159, 88, 227, 223, 95, 8, 10, 209, 71, 155, 19, 244, 39, 151, 221, 160, 232, 147, 185, 17, 168, 33, 30, 80, 97, 94, 111, 90, 145, 23],
			[60, 63, 34, 232, 97, 176, 18, 120, 81, 178, 12, 69, 219, 238, 113, 125, 26, 228, 253, 183, 174, 26, 138, 208, 111, 64, 64, 41, 244, 124, 121, 67],
			[40, 148, 254, 89, 9, 137, 110, 100, 156, 123, 146, 165, 201, 220, 254, 199, 164, 120, 52, 58, 234, 170, 210, 158, 121, 241, 68, 27, 79, 59, 113, 37],
			[10, 80, 189, 80, 152, 191, 196, 83, 56, 254, 215, 66, 252, 122, 147, 90, 255, 158, 208, 88, 197, 55, 123, 32, 17, 101, 133, 144, 127, 16, 98, 1],
			[46, 105, 172, 145, 220, 43, 62, 84, 175, 210, 215, 71, 54, 231, 223, 217, 95, 170, 30, 115, 141, 171, 6, 108, 128, 50, 137, 128, 199, 201, 7, 110],
			[34, 67, 70, 237, 99, 250, 41, 140, 128, 100, 237, 222, 206, 7, 18, 51, 3, 66, 165, 15, 47, 21, 42, 95, 175, 180, 84, 240, 9, 165, 104, 85],
			[86, 15, 189, 117, 179, 219, 150, 239, 113, 227, 59, 97, 96, 14, 63, 55, 169, 38, 64, 8, 135, 218, 170, 174, 56, 13, 54, 54, 148, 156, 7, 103],
			[106, 217, 75, 166, 62, 43, 95, 39, 205, 242, 178, 147, 7, 109, 3, 214, 253, 255, 44, 20, 164, 97, 54, 104, 211, 243, 117, 150, 167, 140, 152, 71],
			[54, 149, 171, 208, 232, 116, 221, 99, 156, 141, 102, 199, 185, 226, 175, 117, 139, 91, 54, 222, 54, 187, 1, 240, 233, 80, 72, 207, 181, 224, 15, 104],
			[50, 174, 189, 199, 130, 120, 182, 27, 121, 74, 196, 214, 54, 179, 189, 241, 91, 1, 232, 195, 235, 11, 118, 71, 106, 115, 21, 53, 107, 92, 173, 13],
			[248, 50, 93, 17, 160, 222, 207, 148, 89, 28, 188, 52, 219, 39, 38, 73, 24, 224, 147, 207, 156, 221, 0, 146, 208, 108, 78, 134, 97, 111, 28, 41],
			[196, 252, 84, 183, 173, 5, 166, 238, 111, 47, 225, 171, 174, 86, 2, 197, 161, 240, 88, 149, 207, 167, 191, 117, 184, 97, 188, 245, 46, 62, 24, 99],
			[152, 102, 212, 80, 61, 5, 186, 40, 174, 224, 52, 123, 31, 99, 129, 168, 38, 158, 80, 205, 38, 8, 190, 75, 155, 233, 112, 115, 234, 155, 158, 5],
			[58, 244, 16, 159, 67, 195, 93, 65, 105, 111, 153, 149, 45, 112, 230, 188, 137, 80, 77, 197, 83, 61, 191, 24, 151, 55, 187, 203, 215, 135, 96, 97],
			[188, 163, 103, 207, 165, 67, 118, 65, 78, 154, 254, 205, 53, 215, 163, 42, 23, 1, 31, 210, 108, 134, 202, 237, 146, 247, 187, 188, 11, 238, 11, 127],
			[70, 125, 148, 246, 12, 162, 254, 200, 189, 252, 132, 57, 38, 8, 141, 245, 173, 39, 79, 235, 74, 140, 44, 208, 70, 92, 168, 203, 120, 245, 76, 114],
			[44, 240, 131, 139, 5, 251, 24, 39, 24, 222, 133, 149, 37, 250, 30, 109, 83, 213, 87, 229, 252, 246, 49, 238, 159, 244, 76, 97, 152, 16, 212, 59],
			[138, 247, 46, 8, 175, 253, 239, 75, 125, 166, 137, 80, 188, 72, 94, 147, 57, 41, 40, 23, 129, 252, 18, 213, 36, 233, 140, 140, 30, 144, 164, 29],
			[20, 70, 102, 216, 66, 208, 224, 194, 67, 50, 123, 232, 185, 119, 254, 139, 64, 238, 36, 57, 24, 65, 14, 129, 107, 127, 195, 178, 199, 159, 116, 102],
			[200, 131, 83, 61, 79, 131, 122, 150, 17, 115, 59, 190, 222, 176, 212, 32, 178, 87, 61, 28, 144, 55, 39, 59, 72, 181, 35, 55, 104, 248, 95, 8],
			[124, 213, 155, 165, 255, 79, 185, 97, 149, 226, 204, 185, 234, 204, 215, 139, 255, 152, 46, 15, 21, 219, 126, 148, 45, 114, 209, 185, 87, 162, 252, 10],
			[112, 189, 233, 173, 82, 193, 14, 226, 75, 136, 20, 76, 97, 47, 14, 86, 208, 211, 183, 153, 91, 217, 224, 84, 17, 112, 224, 111, 46, 127, 199, 8],
			[56, 15, 250, 13, 153, 166, 81, 158, 10, 180, 216, 160, 140, 45, 96, 255, 90, 140, 119, 98, 199, 158, 20, 138, 230, 238, 137, 145, 112, 1, 0, 68],
			[62, 110, 138, 244, 155, 221, 46, 135, 254, 143, 195, 97, 196, 10, 114, 182, 95, 193, 193, 238, 177, 161, 79, 135, 6, 67, 54, 244, 45, 223, 231, 3],
			[190, 126, 211, 122, 134, 233, 155, 156, 17, 151, 255, 143, 163, 165, 228, 182, 64, 59, 84, 1, 150, 246, 205, 9, 175, 47, 188, 67, 234, 154, 87, 115],
			[156, 8, 170, 109, 173, 183, 172, 39, 165, 150, 128, 2, 57, 201, 163, 99, 200, 160, 148, 206, 213, 196, 98, 132, 153, 72, 241, 15, 81, 45, 158, 27],
			[238, 158, 10, 156, 237, 29, 152, 9, 5, 107, 74, 220, 168, 210, 36, 234, 60, 53, 154, 185, 175, 31, 182, 152, 96, 40, 254, 129, 110, 55, 102, 90],
			[222, 136, 73, 91, 148, 50, 65, 218, 20, 17, 179, 20, 86, 14, 220, 181, 27, 201, 144, 98, 219, 220, 77, 207, 144, 107, 172, 12, 72, 82, 244, 52],
			[20, 188, 115, 8, 240, 253, 101, 118, 31, 236, 245, 236, 16, 75, 180, 56, 238, 70, 125, 153, 10, 248, 72, 55, 204, 56, 122, 105, 222, 73, 168, 95],
			[134, 140, 213, 79, 174, 161, 160, 228, 88, 54, 99, 91, 43, 246, 88, 115, 52, 54, 236, 105, 197, 86, 125, 101, 27, 229, 146, 57, 44, 187, 105, 220],
			[100, 103, 253, 78, 112, 56, 185, 37, 194, 66, 35, 87, 56, 13, 140, 192, 197, 241, 125, 39, 47, 99, 154, 248, 252, 253, 31, 17, 86, 222, 112, 64],
			[176, 119, 188, 76, 14, 254, 156, 6, 250, 209, 36, 141, 91, 39, 90, 121, 157, 44, 229, 114, 204, 187, 146, 96, 27, 172, 36, 104, 210, 159, 228, 75],
			[212, 72, 42, 216, 212, 46, 156, 252, 128, 249, 248, 10, 9, 55, 100, 74, 36, 62, 89, 139, 239, 130, 62, 59, 33, 68, 68, 84, 53, 197, 54, 35],
			[94, 229, 69, 146, 105, 249, 76, 245, 52, 214, 99, 26, 51, 45, 212, 153, 4, 169, 75, 56, 71, 104, 117, 103, 206, 172, 77, 215, 76, 187, 37, 18],
			[168, 7, 6, 72, 246, 228, 59, 125, 138, 143, 16, 65, 139, 105, 97, 48, 210, 4, 108, 16, 100, 95, 16, 8, 93, 232, 14, 96, 152, 184, 95, 9],
			[12, 30, 86, 186, 160, 124, 128, 173, 10, 212, 212, 241, 151, 236, 105, 29, 17, 4, 103, 1, 12, 168, 194, 86, 71, 57, 145, 157, 113, 209, 9, 124],
			[162, 97, 27, 101, 196, 115, 166, 134, 30, 13, 237, 211, 142, 107, 20, 138, 87, 77, 165, 10, 133, 77, 181, 60, 105, 241, 234, 73, 65, 240, 214, 40],
			[168, 243, 128, 29, 140, 120, 224, 144, 194, 1, 238, 189, 86, 169, 82, 167, 233, 13, 83, 92, 237, 86, 132, 253, 211, 253, 103, 106, 154, 207, 75, 68],
			[76, 228, 33, 55, 12, 240, 37, 125, 134, 150, 24, 236, 37, 195, 36, 237, 76, 108, 127, 101, 40, 146, 151, 163, 193, 52, 51, 44, 33, 46, 53, 11],
			[60, 51, 69, 125, 109, 17, 237, 123, 60, 82, 245, 245, 89, 208, 48, 121, 2, 208, 151, 80, 79, 101, 160, 185, 87, 194, 175, 234, 146, 246, 63, 28],
			[186, 80, 165, 140, 50, 132, 33, 151, 29, 245, 67, 142, 199, 59, 10, 187, 95, 78, 69, 71, 166, 254, 108, 31, 9, 9, 6, 230, 11, 71, 49, 67],
			[166, 148, 132, 242, 177, 14, 194, 241, 222, 161, 147, 148, 66, 61, 87, 111, 145, 198, 181, 171, 35, 21, 179, 137, 244, 225, 8, 188, 240, 170, 40, 64],
		]
			.into_iter()
			.map(|acc|
				AccountId::from(acc)
			).for_each(|acc| {
				if !Balances::unreserve(&acc, to_unreserve).is_zero() {
					failure += 1;
				};
			});
		frame_support::debug::info!("Migration to fix voters happened. Accounts with inaccurate reserved amount: {}", failure);
		<Runtime as frame_system::Config>::MaximumBlockWeight::get()
	}
}

pub struct CustomOnRuntimeUpgrade;
impl frame_support::traits::OnRuntimeUpgrade for CustomOnRuntimeUpgrade {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		0
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
		Babe: pallet_babe::{Module, Call, Storage, Config, Inherent, ValidateUnsigned} = 1,

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
		Treasury: pallet_treasury::{Module, Call, Storage, Event<T>} = 18,

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
	}
}

/// The address format for describing accounts.
pub type Address = AccountId;
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
	FixCouncilHistoricalVotes,
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

		fn availability_cores() -> Vec<CoreState<BlockNumber>> {
			Vec::new()
		}

		fn full_validation_data(_: Id, _: OccupiedCoreAssumption)
			-> Option<ValidationData<BlockNumber>> {
			None
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

		fn current_epoch_start() -> babe_primitives::SlotNumber {
			Babe::current_epoch_start()
		}

		fn generate_key_ownership_proof(
			_slot_number: babe_primitives::SlotNumber,
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
			add_benchmark!(params, batches, claims, Claims);
			// Substrate
			add_benchmark!(params, batches, pallet_balances, Balances);
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
		let raw_fee = WeightToFee::calc(&MaximumBlockWeight::get());

		println!(
			"Full Block weight == {} // WeightToFee(full_block) == {} plank",
			MaximumBlockWeight::get(),
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
