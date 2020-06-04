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
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! The Polkadot runtime. This can be compiled with `#[no_std]`, ready for Wasm.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit="256"]

use runtime_common::{attestations, claims, parachains, registrar, slots,
	impls::{CurrencyToVoteHandler, TargetedFeeAdjustment, ToAuthor},
	NegativeImbalance, BlockHashCount, MaximumBlockWeight, AvailableBlockRatio,
	MaximumBlockLength, BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight,
	MaximumExtrinsicWeight, TransactionCallFilter,
};

use sp_std::prelude::*;
use sp_core::u32_trait::{_1, _2, _3, _4, _5};
use codec::{Encode, Decode};
use primitives::{
	AccountId, AccountIndex, Balance, BlockNumber, Hash, Nonce, Signature, Moment,
	parachain::{self, ActiveParas, AbridgedCandidateReceipt, SigningContext},
};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys, ModuleId,
	ApplyExtrinsicResult, KeyTypeId, Percent, Permill, Perbill, Perquintill, PerThing,
	transaction_validity::{
		TransactionValidity, TransactionSource, TransactionPriority,
	},
	curve::PiecewiseLinear,
	traits::{
		BlakeTwo256, Block as BlockT, OpaqueKeys, ConvertInto, IdentityLookup,
		Extrinsic as ExtrinsicT, SaturatedConversion, Verify,
	},
};
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::RuntimeString;
use version::RuntimeVersion;
use grandpa::{AuthorityId as GrandpaId, fg_primitives};
#[cfg(any(feature = "std", test))]
use version::NativeVersion;
use sp_core::OpaqueMetadata;
use sp_staking::SessionIndex;
use frame_support::{
	parameter_types, construct_runtime, debug,
	traits::{KeyOwnerProofSystem, SplitTwoWays, Randomness, LockIdentifier, Filter},
	weights::Weight,
};
use im_online::sr25519::AuthorityId as ImOnlineId;
use authority_discovery_primitives::AuthorityId as AuthorityDiscoveryId;
use transaction_payment_rpc_runtime_api::RuntimeDispatchInfo;
use session::historical as session_historical;
use static_assertions::const_assert;

#[cfg(feature = "std")]
pub use staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use timestamp::Call as TimestampCall;
pub use balances::Call as BalancesCall;
pub use attestations::{Call as AttestationsCall, MORE_ATTESTATIONS_IDENTIFIER};
pub use parachains::Call as ParachainsCall;

/// Constant values used within the runtime.
pub mod constants;
use constants::{time::*, currency::*, fee::*};

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

// Polkadot version identifier;
/// Runtime version (Polkadot).
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("polkadot"),
	impl_name: create_runtime_str!("parity-polkadot"),
	authoring_version: 0,
	spec_version: 2,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 0,
};

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion {
		runtime_version: VERSION,
		can_author_with: Default::default(),
	}
}

pub struct IsCallable;
impl Filter<Call> for IsCallable {
	fn filter(call: &Call) -> bool {
		match call {
			Call::Parachains(parachains::Call::set_heads(..)) => true,

			// Governance stuff
			Call::Democracy(_) | Call::Council(_) | Call::TechnicalCommittee(_) |
			Call::ElectionsPhragmen(_) | Call::TechnicalMembership(_) | Call::Treasury(_) |
			// Parachains stuff
			Call::Parachains(_) | Call::Attestations(_) | Call::Slots(_) | Call::Registrar(_) |
			// Balances and Vesting's transfer (which can be used to transfer)
			Call::Balances(_) | Call::Vesting(vesting::Call::vested_transfer(..)) |
			Call::Indices(indices::Call::transfer(..)) =>
				false,

			// These modules are all allowed to be called by transactions:
			Call::System(_) | Call::Scheduler(_) | Call::Indices(_) |
			Call::Babe(_) | Call::Timestamp(_) |
			Call::Authorship(_) | Call::Staking(_) | Call::Offences(_) |
			Call::Session(_) | Call::FinalityTracker(_) | Call::Grandpa(_) | Call::ImOnline(_) |
			Call::AuthorityDiscovery(_) |
			Call::Utility(_) | Call::Claims(_) | Call::Vesting(_) | Call::Sudo(_) |
			Call::Identity(_) =>
				true,
		}
	}
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
}

impl system::Trait for Runtime {
	type Origin = Origin;
	type Call = Call;
	type Index = Nonce;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<AccountId>;
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
	type ModuleToIndex = ModuleToIndex;
	type AccountData = balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
}

impl scheduler::Trait for Runtime {
	type Event = Event;
	type Origin = Origin;
	type Call = Call;
	type MaximumWeight = MaximumBlockWeight;
}

parameter_types! {
	pub const EpochDuration: u64 = EPOCH_DURATION_IN_BLOCKS as u64;
	pub const ExpectedBlockTime: Moment = MILLISECS_PER_BLOCK;
}

impl babe::Trait for Runtime {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;

	// session module is the trigger
	type EpochChangeTrigger = babe::ExternalTrigger;
}

parameter_types! {
	pub const IndexDeposit: Balance = 10 * DOLLARS;
}

impl indices::Trait for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type Event = Event;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 100 * CENTS;
}

/// Splits fees 80/20 between treasury and block author.
pub type DealWithFees = SplitTwoWays<
	Balance,
	NegativeImbalance<Runtime>,
	_4, Treasury,   		// 4 parts (80%) goes to the treasury.
	_1, ToAuthor<Runtime>,   	// 1 part (20%) goes to the block author.
>;

impl balances::Trait for Runtime {
	type Balance = Balance;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
}

parameter_types! {
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
	pub const TargetBlockFullness: Perquintill = Perquintill::from_percent(25);
}

// for a sane configuration, this should always be less than `AvailableBlockRatio`.
const_assert!(
	TargetBlockFullness::get().deconstruct() <
	(AvailableBlockRatio::get().deconstruct() as <Perquintill as PerThing>::Inner)
		* (<Perquintill as PerThing>::ACCURACY / <Perbill as PerThing>::ACCURACY as <Perquintill as PerThing>::Inner)
);

impl transaction_payment::Trait for Runtime {
	type Currency = Balances;
	type OnTransactionPayment = DealWithFees;
	type TransactionByteFee = TransactionByteFee;
	type WeightToFee = WeightToFee;
	type FeeMultiplierUpdate = TargetedFeeAdjustment<TargetBlockFullness, Self>;
}

parameter_types! {
	pub const MinimumPeriod: u64 = SLOT_DURATION / 2;
}
impl timestamp::Trait for Runtime {
	type Moment = u64;
	type OnTimestampSet = Babe;
	type MinimumPeriod = MinimumPeriod;
}

parameter_types! {
	pub const UncleGenerations: u32 = 0;
}

// TODO: substrate#2986 implement this properly
impl authorship::Trait for Runtime {
	type FindAuthor = session::FindAccountFromAuthorIndex<Self, Babe>;
	type UncleGenerations = UncleGenerations;
	type FilterUncle = ();
	type EventHandler = (Staking, ImOnline);
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub im_online: ImOnline,
		pub parachain_validator: Parachains,
		pub authority_discovery: AuthorityDiscovery,
	}
}

parameter_types! {
	pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
}

impl session::Trait for Runtime {
	type Event = Event;
	type ValidatorId = AccountId;
	type ValidatorIdOf = staking::StashOf<Self>;
	type ShouldEndSession = Babe;
	type NextSessionRotation = Babe;
	type SessionManager = session::historical::NoteHistoricalRoot<Self, Staking>;
	type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
}

impl session::historical::Trait for Runtime {
	type FullIdentification = staking::Exposure<AccountId, Balance>;
	type FullIdentificationOf = staking::ExposureOf<Runtime>;
}

pallet_staking_reward_curve::build! {
	const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
		min_inflation: 0_025_000,
		max_inflation: 0_100_000,
		ideal_stake: 0_500_000,
		falloff: 0_050_000,
		max_piece_count: 40,
		test_precision: 0_005_000,
	);
}

parameter_types! {
	// Six sessions in an era (24 hours).
	pub const SessionsPerEra: SessionIndex = 6;
	// 28 eras for unbonding (28 days).
	pub const BondingDuration: staking::EraIndex = 28;
	pub const SlashDeferDuration: staking::EraIndex = 28;
	pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	pub const MaxNominatorRewardedPerValidator: u32 = 64;
	// quarter of the last session will be for election.
	pub const ElectionLookahead: BlockNumber = EPOCH_DURATION_IN_BLOCKS / 16;
	pub const MaxIterations: u32 = 10;
	pub MinSolutionScoreBump: Perbill = Perbill::from_rational_approximation(5u32, 10_000);
}

impl staking::Trait for Runtime {
	type Currency = Balances;
	type UnixTime = Timestamp;
	type CurrencyToVote = CurrencyToVoteHandler<Self>;
	type RewardRemainder = Treasury;
	type Event = Event;
	type Slash = Treasury;
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SlashDeferDuration = SlashDeferDuration;
	// A super-majority of the council can cancel the slash.
	type SlashCancelOrigin = collective::EnsureProportionAtLeast<_3, _4, AccountId, CouncilCollective>;
	type SessionInterface = Self;
	type RewardCurve = RewardCurve;
	type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
	type NextNewSession = Session;
	type ElectionLookahead = ElectionLookahead;
	type Call = Call;
	type UnsignedPriority = StakingUnsignedPriority;
	type MaxIterations = MaxIterations;
	type MinSolutionScoreBump = MinSolutionScoreBump;
}

const fn deposit(items: u32, bytes: u32) -> Balance {
	items as Balance * 15 * CENTS + (bytes as Balance) * 6 * CENTS
}

parameter_types! {
	// Minimum 4 CENTS/byte
	pub const BasicDeposit: Balance = deposit(1, 258);
	pub const FieldDeposit: Balance = deposit(0, 66);
	pub const SubAccountDeposit: Balance = deposit(1, 53);
	pub const MaxSubAccounts: u32 = 100;
	pub const MaxAdditionalFields: u32 = 100;
	pub const MaxRegistrars: u32 = 20;
}

impl identity::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type BasicDeposit = BasicDeposit;
	type FieldDeposit = FieldDeposit;
	type SubAccountDeposit = SubAccountDeposit;
	type MaxSubAccounts = MaxSubAccounts;
	type MaxAdditionalFields = MaxAdditionalFields;
	type MaxRegistrars = MaxRegistrars;
	type Slashed = Treasury;
	type ForceOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type RegistrarOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
}

parameter_types! {
	pub const LaunchPeriod: BlockNumber = 28 * DAYS;
	pub const VotingPeriod: BlockNumber = 28 * DAYS;
	pub const FastTrackVotingPeriod: BlockNumber = 3 * HOURS;
	pub const MinimumDeposit: Balance = 100 * DOLLARS;
	pub const EnactmentPeriod: BlockNumber = 8 * DAYS;
	pub const CooloffPeriod: BlockNumber = 7 * DAYS;
	// One cent: $10,000 / MB
	pub const PreimageByteDeposit: Balance = 1 * CENTS;
	pub const InstantAllowed: bool = true;
	pub const MaxVotes: u32 = 100;
}

impl democracy::Trait for Runtime {
	type Proposal = Call;
	type Event = Event;
	type Currency = Balances;
	type EnactmentPeriod = EnactmentPeriod;
	type LaunchPeriod = LaunchPeriod;
	type VotingPeriod = VotingPeriod;
	type MinimumDeposit = MinimumDeposit;
	/// A straight majority of the council can decide what their next motion is.
	type ExternalOrigin = collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>;
	/// A 60% super-majority can have the next scheduled referendum be a straight majority-carries vote.
	type ExternalMajorityOrigin = collective::EnsureProportionAtLeast<_3, _5, AccountId, CouncilCollective>;
	/// A unanimous council can have the next scheduled referendum be a straight default-carries
	/// (NTB) vote.
	type ExternalDefaultOrigin = collective::EnsureProportionAtLeast<_1, _1, AccountId, CouncilCollective>;
	/// Two thirds of the technical committee can have an ExternalMajority/ExternalDefault vote
	/// be tabled immediately and with a shorter voting/enactment period.
	type FastTrackOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, TechnicalCollective>;
	type InstantOrigin = collective::EnsureProportionAtLeast<_1, _1, AccountId, TechnicalCollective>;
	type InstantAllowed = InstantAllowed;
	type FastTrackVotingPeriod = FastTrackVotingPeriod;
	// To cancel a proposal which has been passed, 2/3 of the council must agree to it.
	type CancellationOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilCollective>;
	// Any single technical committee member may veto a coming council proposal, however they can
	// only do it once and it lasts only for the cooloff period.
	type VetoOrigin = collective::EnsureMember<AccountId, TechnicalCollective>;
	type CooloffPeriod = CooloffPeriod;
	type PreimageByteDeposit = PreimageByteDeposit;
	type OperationalPreimageOrigin = collective::EnsureMember<AccountId, CouncilCollective>;
	type Slash = Treasury;
	type Scheduler = Scheduler;
	type MaxVotes = MaxVotes;
}

parameter_types! {
	pub const CouncilMotionDuration: BlockNumber = 7 * DAYS;
	pub const CouncilMaxProposals: u32 = 100;
}

type CouncilCollective = collective::Instance1;
impl collective::Trait<CouncilCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
	type MotionDuration = CouncilMotionDuration;
	type MaxProposals = CouncilMaxProposals;
}

parameter_types! {
	pub const CandidacyBond: Balance = 100 * DOLLARS;
	pub const VotingBond: Balance = 5 * DOLLARS;
	/// Weekly council elections initially, later monthly.
	pub const TermDuration: BlockNumber = 7 * DAYS;
	/// 13 members initially, to be increased to 23 eventually.
	pub const DesiredMembers: u32 = 13;
	pub const DesiredRunnersUp: u32 = 20;
	pub const ElectionsPhragmenModuleId: LockIdentifier = *b"phrelect";
}
// Make sure that there are no more than MAX_MEMBERS members elected via phragmen.
const_assert!(DesiredMembers::get() <= collective::MAX_MEMBERS);

impl elections_phragmen::Trait for Runtime {
	type Event = Event;
	type ModuleId = ElectionsPhragmenModuleId;
	type Currency = Balances;
	type ChangeMembers = Council;
	type InitializeMembers = Council;
	type CurrencyToVote = CurrencyToVoteHandler<Self>;
	type CandidacyBond = CandidacyBond;
	type VotingBond = VotingBond;
	type LoserCandidate = Treasury;
	type BadReport = Treasury;
	type KickedMember = Treasury;
	type DesiredMembers = DesiredMembers;
	type DesiredRunnersUp = DesiredRunnersUp;
	type TermDuration = TermDuration;
}

parameter_types! {
	pub const TechnicalMotionDuration: BlockNumber = 7 * DAYS;
	pub const TechnicalMaxProposals: u32 = 100;
}

type TechnicalCollective = collective::Instance2;
impl collective::Trait<TechnicalCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
	type MotionDuration = TechnicalMotionDuration;
	type MaxProposals = TechnicalMaxProposals;
}

impl membership::Trait<membership::Instance1> for Runtime {
	type Event = Event;
	type AddOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type RemoveOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type SwapOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type ResetOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type PrimeOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type MembershipInitialized = TechnicalCommittee;
	type MembershipChanged = TechnicalCommittee;
}

parameter_types! {
	pub const ProposalBond: Permill = Permill::from_percent(5);
	pub const ProposalBondMinimum: Balance = 100 * DOLLARS;
	pub const SpendPeriod: BlockNumber = 24 * DAYS;
	pub const Burn: Permill = Permill::from_percent(1);
	pub const TreasuryModuleId: ModuleId = ModuleId(*b"py/trsry");

	pub const TipCountdown: BlockNumber = 1 * DAYS;
	pub const TipFindersFee: Percent = Percent::from_percent(20);
	pub const TipReportDepositBase: Balance = 1 * DOLLARS;
	pub const TipReportDepositPerByte: Balance = 1 * CENTS;
}

impl treasury::Trait for Runtime {
	type ModuleId = TreasuryModuleId;
	type Currency = Balances;
	type ApproveOrigin = collective::EnsureProportionAtLeast<_3, _5, AccountId, CouncilCollective>;
	type RejectOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type Tippers = ElectionsPhragmen;
	type TipCountdown = TipCountdown;
	type TipFindersFee = TipFindersFee;
	type TipReportDepositBase = TipReportDepositBase;
	type TipReportDepositPerByte = TipReportDepositPerByte;
	type Event = Event;
	type ProposalRejection = Treasury;
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type SpendPeriod = SpendPeriod;
	type Burn = Burn;
}

parameter_types! {
	pub OffencesWeightSoftLimit: Weight = Perbill::from_percent(60) * MaximumBlockWeight::get();
}

impl offences::Trait for Runtime {
	type Event = Event;
	type IdentificationTuple = session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = Staking;
	type WeightSoftLimit = OffencesWeightSoftLimit;
}

impl authority_discovery::Trait for Runtime {}

parameter_types! {
	pub const SessionDuration: BlockNumber = EPOCH_DURATION_IN_BLOCKS as _;
}

parameter_types! {
	pub const StakingUnsignedPriority: TransactionPriority = TransactionPriority::max_value() / 2;
	pub const ImOnlineUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl im_online::Trait for Runtime {
	type AuthorityId = ImOnlineId;
	type Event = Event;
	type SessionDuration = SessionDuration;
	type ReportUnresponsiveness = Offences;
	type UnsignedPriority = ImOnlineUnsignedPriority;
}

impl grandpa::Trait for Runtime {
	type Event = Event;
	type Call = Call;

	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		GrandpaId,
	)>>::IdentificationTuple;

	type KeyOwnerProofSystem = Historical;

	type HandleEquivocation = grandpa::EquivocationHandler<
		Self::KeyOwnerIdentification,
		primitives::fisherman::FishermanAppCrypto,
		Runtime,
		Offences,
	>;
}

parameter_types! {
	pub WindowSize: BlockNumber = finality_tracker::DEFAULT_WINDOW_SIZE.into();
	pub ReportLatency: BlockNumber = finality_tracker::DEFAULT_REPORT_LATENCY.into();
}

impl finality_tracker::Trait for Runtime {
	type OnFinalizationStalled = ();
	type WindowSize = WindowSize;
	type ReportLatency = ReportLatency;
}

parameter_types! {
	pub const AttestationPeriod: BlockNumber = 50;
}

impl attestations::Trait for Runtime {
	type AttestationPeriod = AttestationPeriod;
	type ValidatorIdentities = parachains::ValidatorIdentities<Runtime>;
	type RewardAttestation = Staking;
}

parameter_types! {
	pub const MaxCodeSize: u32 = 10 * 1024 * 1024; // 10 MB
	pub const MaxHeadDataSize: u32 = 20 * 1024; // 20 KB

	pub const ValidationUpgradeFrequency: BlockNumber = 7 * DAYS;
	pub const ValidationUpgradeDelay: BlockNumber = 1 * DAYS;
	pub const SlashPeriod: BlockNumber = 28 * DAYS;
}

impl parachains::Trait for Runtime {
	type AuthorityId = primitives::fisherman::FishermanAppCrypto;
	type Origin = Origin;
	type Call = Call;
	type ParachainCurrency = Balances;
	type BlockNumberConversion = sp_runtime::traits::Identity;
	type Randomness = RandomnessCollectiveFlip;
	type ActiveParachains = Registrar;
	type Registrar = Registrar;
	type MaxCodeSize = MaxCodeSize;
	type MaxHeadDataSize = MaxHeadDataSize;

	type ValidationUpgradeFrequency = ValidationUpgradeFrequency;
	type ValidationUpgradeDelay = ValidationUpgradeDelay;
	type SlashPeriod = SlashPeriod;

	type Proof = sp_session::MembershipProof;
	type KeyOwnerProofSystem = session::historical::Module<Self>;
	type IdentificationTuple = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, Vec<u8>)>>::IdentificationTuple;
	type ReportOffence = Offences;
	type BlockHashConversion = sp_runtime::traits::Identity;
}

/// Submits a transaction with the node's public and signature type. Adheres to the signed extension
/// format of the chain.
impl<LocalCall> system::offchain::CreateSignedTransaction<LocalCall> for Runtime where
	Call: From<LocalCall>,
{
	fn create_transaction<C: system::offchain::AppCrypto<Self::Public, Self::Signature>>(
		call: Call,
		public: <Signature as Verify>::Signer,
		account: AccountId,
		nonce: <Runtime as system::Trait>::Index,
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
			TransactionCallFilter::<IsCallable, Call>::new(),
			system::CheckSpecVersion::<Runtime>::new(),
			system::CheckTxVersion::<Runtime>::new(),
			system::CheckGenesis::<Runtime>::new(),
			system::CheckEra::<Runtime>::from(generic::Era::mortal(period, current_block)),
			system::CheckNonce::<Runtime>::from(nonce),
			system::CheckWeight::<Runtime>::new(),
			transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
			registrar::LimitParathreadCommits::<Runtime>::new(),
			parachains::ValidateDoubleVoteReports::<Runtime>::new(),
			grandpa::ValidateEquivocationReport::<Runtime>::new(),
			claims::PrevalidateAttests::<Runtime>::new(),
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

impl system::offchain::SigningTypes for Runtime {
	type Public = <Signature as Verify>::Signer;
	type Signature = Signature;
}

impl<C> system::offchain::SendTransactionTypes<C> for Runtime where Call: From<C> {
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = Call;
}

parameter_types! {
	pub const ParathreadDeposit: Balance = 500 * DOLLARS;
	pub const QueueSize: usize = 2;
	pub const MaxRetries: u32 = 3;
}

impl registrar::Trait for Runtime {
	type Event = Event;
	type Origin = Origin;
	type Currency = Balances;
	type ParathreadDeposit = ParathreadDeposit;
	type SwapAux = Slots;
	type QueueSize = QueueSize;
	type MaxRetries = MaxRetries;
}

parameter_types! {
	pub const LeasePeriod: BlockNumber = 100_000;
	pub const EndingPeriod: BlockNumber = 1000;
}

impl slots::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Parachains = Registrar;
	type EndingPeriod = EndingPeriod;
	type LeasePeriod = LeasePeriod;
	type Randomness = RandomnessCollectiveFlip;
}

parameter_types! {
	pub Prefix: &'static [u8] = b"Pay DOTs to the Polkadot account:";
}

impl claims::Trait for Runtime {
	type Event = Event;
	type VestingSchedule = Vesting;
	type Prefix = Prefix;
}

parameter_types! {
	pub const MinVestedTransfer: Balance = 100 * DOLLARS;
}

impl vesting::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type BlockNumberToBalance = ConvertInto;
	type MinVestedTransfer = MinVestedTransfer;
}

parameter_types! {
	// One storage item; value is size 4+4+16+32 bytes = 56 bytes.
	pub const MultisigDepositBase: Balance = 30 * CENTS;
	// Additional storage item size of 32 bytes.
	pub const MultisigDepositFactor: Balance = 5 * CENTS;
	pub const MaxSignatories: u16 = 100;
}

impl utility::Trait for Runtime {
	type Event = Event;
	type Call = Call;
	type Currency = Balances;
	type MultisigDepositBase = MultisigDepositBase;
	type MultisigDepositFactor = MultisigDepositFactor;
	type MaxSignatories = MaxSignatories;
	type IsCallable = IsCallable;
}

impl sudo::Trait for Runtime {
	type Event = Event;
	type Call = Call;
}

construct_runtime! {
	pub enum Runtime where
		Block = Block,
		NodeBlock = primitives::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		// Basic stuff; balances is uncallable initially.
		System: system::{Module, Call, Storage, Config, Event<T>},
		RandomnessCollectiveFlip: randomness_collective_flip::{Module, Storage},
		Scheduler: scheduler::{Module, Call, Storage, Event<T>},

		// Must be before session.
		Babe: babe::{Module, Call, Storage, Config, Inherent(Timestamp)},

		Timestamp: timestamp::{Module, Call, Storage, Inherent},
		Indices: indices::{Module, Call, Storage, Config<T>, Event<T>},
		Balances: balances::{Module, Call, Storage, Config<T>, Event<T>},
		TransactionPayment: transaction_payment::{Module, Storage},

		// Consensus support.
		Authorship: authorship::{Module, Call, Storage},
		Staking: staking::{Module, Call, Storage, Config<T>, Event<T>, ValidateUnsigned},
		Offences: offences::{Module, Call, Storage, Event},
		Historical: session_historical::{Module},
		Session: session::{Module, Call, Storage, Event, Config<T>},
		FinalityTracker: finality_tracker::{Module, Call, Storage, Inherent},
		Grandpa: grandpa::{Module, Call, Storage, Config, Event},
		ImOnline: im_online::{Module, Call, Storage, Event<T>, ValidateUnsigned, Config<T>},
		AuthorityDiscovery: authority_discovery::{Module, Call, Config},

		// Governance stuff; uncallable initially. Calls should be uncommented once we're ready to
		// enable governance.
		Democracy: democracy::{Module, Call, Storage, Config, Event<T>},
		Council: collective::<Instance1>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>},
		TechnicalCommittee: collective::<Instance2>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>},
		ElectionsPhragmen: elections_phragmen::{Module, Call, Storage, Event<T>, Config<T>},
		TechnicalMembership: membership::<Instance1>::{Module, Call, Storage, Event<T>, Config<T>},
		Treasury: treasury::{Module, Call, Storage, Event<T>},

		// Parachains stuff; slots are disabled (no auctions initially). The rest are safe as they
		// have no public dispatchables. Disabled `Call` on all of them, but this should be
		// uncommented once we're ready to start parachains.
		Parachains: parachains::{Module, Call, Storage, Config, Inherent, Origin},
		Attestations: attestations::{Module, Call, Storage},
		Slots: slots::{Module, Call, Storage, Event<T>},
		Registrar: registrar::{Module, Call, Storage, Event, Config<T>},

		// Claims. Usable initially.
		Claims: claims::{Module, Call, Storage, Event<T>, Config<T>, ValidateUnsigned},
		// Vesting. Usable initially, but removed once all vesting is finished.
		Vesting: vesting::{Module, Call, Storage, Event<T>, Config<T>},
		// Cunning utilities. Usable initially.
		Utility: utility::{Module, Call, Storage, Event<T>},

		// Sudo. Last module. Usable initially, but removed once governance enabled.
		Sudo: sudo::{Module, Call, Storage, Config<T>, Event<T>},

		// Identity. Late addition.
		Identity: identity::{Module, Call, Storage, Event<T>},
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
	TransactionCallFilter<IsCallable, Call>,
	system::CheckSpecVersion<Runtime>,
	system::CheckTxVersion<Runtime>,
	system::CheckGenesis<Runtime>,
	system::CheckEra<Runtime>,
	system::CheckNonce<Runtime>,
	system::CheckWeight<Runtime>,
	transaction_payment::ChargeTransactionPayment<Runtime>,
	registrar::LimitParathreadCommits<Runtime>,
	parachains::ValidateDoubleVoteReports<Runtime>,
	grandpa::ValidateEquivocationReport<Runtime>,
	claims::PrevalidateAttests<Runtime>,
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<Address, Call, Signature, SignedExtra>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Nonce, Call>;
/// Executive: handles dispatch to the various modules.
pub type Executive = executive::Executive<Runtime, Block, system::ChainContext<Runtime>, Runtime, AllModules>;
/// The payload being signed in transactions.
pub type SignedPayload = generic::SignedPayload<Call, SignedExtra>;

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

	impl parachain::ParachainHost<Block> for Runtime {
		fn validators() -> Vec<parachain::ValidatorId> {
			Parachains::authorities()
		}
		fn duty_roster() -> parachain::DutyRoster {
			Parachains::calculate_duty_roster().0
		}
		fn active_parachains() -> Vec<(parachain::Id, Option<(parachain::CollatorId, parachain::Retriable)>)> {
			Registrar::active_paras()
		}
		fn global_validation_schedule() -> parachain::GlobalValidationSchedule {
			Parachains::global_validation_schedule()
		}
		fn local_validation_data(id: parachain::Id) -> Option<parachain::LocalValidationData> {
			Parachains::current_local_validation_data(&id)
		}
		fn parachain_code(id: parachain::Id) -> Option<parachain::ValidationCode> {
			Parachains::parachain_code(&id)
		}
		fn get_heads(extrinsics: Vec<<Block as BlockT>::Extrinsic>)
			-> Option<Vec<AbridgedCandidateReceipt>>
		{
			extrinsics
				.into_iter()
				.find_map(|ex| match UncheckedExtrinsic::decode(&mut ex.encode().as_slice()) {
					Ok(ex) => match ex.function {
						Call::Parachains(ParachainsCall::set_heads(heads)) => {
							Some(heads.into_iter().map(|c| c.candidate).collect())
						}
						_ => None,
					}
					Err(_) => None,
				})
		}
		fn signing_context() -> SigningContext {
			Parachains::signing_context()
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> Vec<(GrandpaId, u64)> {
			Grandpa::grandpa_authorities()
		}

		fn submit_report_equivocation_extrinsic(
			equivocation_proof: fg_primitives::EquivocationProof<
				<Block as BlockT>::Hash,
				sp_runtime::traits::NumberFor<Block>,
			>,
			key_owner_proof: fg_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			let key_owner_proof = key_owner_proof.decode()?;

			Grandpa::submit_report_equivocation_extrinsic(
				equivocation_proof,
				key_owner_proof,
			)
		}

		fn generate_key_ownership_proof(
			_set_id: fg_primitives::SetId,
			authority_id: fg_primitives::AuthorityId,
		) -> Option<fg_primitives::OpaqueKeyOwnershipProof> {
			use codec::Encode;

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
				allowed_slots: babe_primitives::AllowedSlots::PrimaryAndSecondaryVRFSlots,
			}
		}

		fn current_epoch_start() -> babe_primitives::SlotNumber {
			Babe::current_epoch_start()
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

	impl system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl transaction_payment_rpc_runtime_api::TransactionPaymentApi<
		Block,
		Balance,
		UncheckedExtrinsic,
	> for Runtime {
		fn query_info(uxt: UncheckedExtrinsic, len: u32) -> RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn dispatch_benchmark(
			pallet: Vec<u8>,
			benchmark: Vec<u8>,
			lowest_range_values: Vec<u32>,
			highest_range_values: Vec<u32>,
			steps: Vec<u32>,
			repeat: u32,
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, RuntimeString> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, add_benchmark};
			// Trying to add benchmarks directly to the Session Pallet caused cyclic dependency issues.
			// To get around that, we separated the Session benchmarks into its own crate, which is why
			// we need these two lines below.
			use pallet_session_benchmarking::Module as SessionBench;
			use pallet_offences_benchmarking::Module as OffencesBench;
			use frame_system_benchmarking::Module as SystemBench;

			impl pallet_session_benchmarking::Trait for Runtime {}
			impl pallet_offences_benchmarking::Trait for Runtime {}
			impl frame_system_benchmarking::Trait for Runtime {}

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&pallet, &benchmark, &lowest_range_values, &highest_range_values, &steps, repeat);
			// Polkadot
			add_benchmark!(params, batches, b"claims", Claims);
			// Substrate
			add_benchmark!(params, batches, b"balances", Balances);
			add_benchmark!(params, batches, b"collective", Council);
			add_benchmark!(params, batches, b"democracy", Democracy);
			add_benchmark!(params, batches, b"elections-phragmen", ElectionsPhragmen);
			add_benchmark!(params, batches, b"im-online", ImOnline);
			add_benchmark!(params, batches, b"offences", OffencesBench::<Runtime>);
			add_benchmark!(params, batches, b"scheduler", Scheduler);
			add_benchmark!(params, batches, b"session", SessionBench::<Runtime>);
			add_benchmark!(params, batches, b"staking", Staking);
			add_benchmark!(params, batches, b"system", SystemBench::<Runtime>);
			add_benchmark!(params, batches, b"timestamp", Timestamp);
			add_benchmark!(params, batches, b"treasury", Treasury);
			add_benchmark!(params, batches, b"vesting", Vesting);

			if batches.is_empty() { return Err("Benchmark not found for this pallet.".into()) }
			Ok(batches)
		}
	}
}
