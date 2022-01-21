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

//! The Kusama runtime. This can be compiled with `#[no_std]`, ready for Wasm.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

use pallet_transaction_payment::CurrencyAdapter;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use primitives::{
	v1::{
		AccountId, AccountIndex, Balance, BlockNumber, CandidateEvent, CommittedCandidateReceipt,
		CoreState, GroupRotationInfo, Hash, Id as ParaId, InboundDownwardMessage,
		InboundHrmpMessage, Moment, Nonce, OccupiedCoreAssumption, PersistedValidationData,
		ScrapedOnChainVotes, Signature, ValidationCode, ValidationCodeHash, ValidatorId,
		ValidatorIndex,
	},
	v2::SessionInfo,
};
use runtime_common::{
	auctions, claims, crowdloan, impls::DealWithFees, paras_registrar, prod_or_fast, slots,
	BlockHashCount, BlockLength, BlockWeights, CurrencyToVote, OffchainSolutionLengthLimit,
	OffchainSolutionWeightLimit, RocksDbWeight, SlowAdjustingFeeUpdate,
};
use sp_core::u32_trait::{_1, _2, _3, _5};
use sp_std::{cmp::Ordering, collections::btree_map::BTreeMap, prelude::*};

use runtime_parachains::{
	configuration as parachains_configuration, disputes as parachains_disputes,
	dmp as parachains_dmp, hrmp as parachains_hrmp, inclusion as parachains_inclusion,
	initializer as parachains_initializer, origin as parachains_origin, paras as parachains_paras,
	paras_inherent as parachains_paras_inherent, reward_points as parachains_reward_points,
	runtime_api_impl::v1 as parachains_runtime_api_impl, scheduler as parachains_scheduler,
	session_info as parachains_session_info, shared as parachains_shared, ump as parachains_ump,
};

use authority_discovery_primitives::AuthorityId as AuthorityDiscoveryId;
use beefy_primitives::crypto::AuthorityId as BeefyId;
use frame_support::{
	construct_runtime, parameter_types,
	traits::{
		Contains, EnsureOneOf, InstanceFilter, KeyOwnerProofSystem, LockIdentifier,
		OnRuntimeUpgrade, PrivilegeCmp,
	},
	weights::Weight,
	PalletId, RuntimeDebug,
};
use frame_system::EnsureRoot;
use pallet_grandpa::{fg_primitives, AuthorityId as GrandpaId};
use pallet_im_online::sr25519::AuthorityId as ImOnlineId;
use pallet_mmr_primitives as mmr;
use pallet_session::historical as session_historical;
use pallet_transaction_payment::{FeeDetails, RuntimeDispatchInfo};
use sp_arithmetic::Perquintill;
use sp_core::OpaqueMetadata;
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{
		AccountIdLookup, BlakeTwo256, Block as BlockT, ConvertInto, Extrinsic as ExtrinsicT,
		OpaqueKeys, SaturatedConversion, Verify,
	},
	transaction_validity::{TransactionPriority, TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, KeyTypeId, Perbill, Percent, Permill,
};
use sp_staking::SessionIndex;
#[cfg(any(feature = "std", test))]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;
use static_assertions::const_assert;

pub use pallet_balances::Call as BalancesCall;
pub use pallet_election_provider_multi_phase::Call as EPMCall;
#[cfg(feature = "std")]
pub use pallet_staking::StakerStatus;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

/// Constant values used within the runtime.
use kusama_runtime_constants::{currency::*, fee::*, time::*};

// Weights used in the runtime.
mod weights;

// Voter bag threshold definitions.
mod bag_thresholds;

// XCM configurations.
pub mod xcm_config;

#[cfg(test)]
mod tests;

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/// Runtime version (Kusama).
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("kusama"),
	impl_name: create_runtime_str!("parity-kusama"),
	authoring_version: 2,
	spec_version: 9140,
	impl_version: 0,
	#[cfg(not(feature = "disable-runtime-api"))]
	apis: RUNTIME_API_VERSIONS,
	#[cfg(feature = "disable-runtime-api")]
	apis: version::create_apis_vec![[]],
	transaction_version: 8,
	state_version: 0,
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

/// Don't allow swaps until parathread story is more mature.
pub struct BaseFilter;
impl Contains<Call> for BaseFilter {
	fn contains(c: &Call) -> bool {
		!matches!(c, Call::Registrar(paras_registrar::Call::swap { .. }))
	}
}

type MoreThanHalfCouncil = EnsureOneOf<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>,
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
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub MaximumSchedulerWeight: Weight = Perbill::from_percent(80) *
		BlockWeights::get().max_block;
	pub const MaxScheduledPerBlock: u32 = 50;
	pub const NoPreimagePostponement: Option<u32> = Some(10);
}

type ScheduleOrigin = EnsureOneOf<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>,
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
	type Event = Event;
	type Origin = Origin;
	type PalletsOrigin = OriginCaller;
	type Call = Call;
	type MaximumWeight = MaximumSchedulerWeight;
	type ScheduleOrigin = ScheduleOrigin;
	type MaxScheduledPerBlock = MaxScheduledPerBlock;
	type WeightInfo = weights::pallet_scheduler::WeightInfo<Runtime>;
	type OriginPrivilegeCmp = OriginPrivilegeCmp;
	type PreimageProvider = Preimage;
	type NoPreimagePostponement = NoPreimagePostponement;
}

parameter_types! {
	pub const PreimageMaxSize: u32 = 4096 * 1024;
	pub const PreimageBaseDeposit: Balance = deposit(2, 64);
	pub const PreimageByteDeposit: Balance = deposit(0, 1);
}

impl pallet_preimage::Config for Runtime {
	type WeightInfo = weights::pallet_preimage::WeightInfo<Runtime>;
	type Event = Event;
	type Currency = Balances;
	type ManagerOrigin = EnsureRoot<AccountId>;
	type MaxSize = PreimageMaxSize;
	type BaseDeposit = PreimageBaseDeposit;
	type ByteDeposit = PreimageByteDeposit;
}

parameter_types! {
	pub EpochDuration: u64 = prod_or_fast!(
		EPOCH_DURATION_IN_SLOTS as u64,
		2 * MINUTES as u64,
		"KSM_EPOCH_DURATION"
	);
	pub const ExpectedBlockTime: Moment = MILLISECS_PER_BLOCK;
	pub ReportLongevity: u64 =
		BondingDuration::get() as u64 * SessionsPerEra::get() as u64 * EpochDuration::get();
}

impl pallet_babe::Config for Runtime {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;

	// session module is the trigger
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;

	type DisabledValidators = Session;

	type KeyOwnerProof = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::IdentificationTuple;

	type KeyOwnerProofSystem = Historical;

	type HandleEquivocation =
		pallet_babe::EquivocationHandler<Self::KeyOwnerIdentification, Offences, ReportLongevity>;

	type WeightInfo = ();

	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const IndexDeposit: Balance = 100 * CENTS;
}

impl pallet_indices::Config for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type Event = Event;
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
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type MaxLocks = MaxLocks;
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
}

parameter_types! {
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
	/// This value increases the priority of `Operational` transactions by adding
	/// a "virtual tip" that's equal to the `OperationalFeeMultiplier * final_fee`.
	pub const OperationalFeeMultiplier: u8 = 5;
}

impl pallet_transaction_payment::Config for Runtime {
	type OnChargeTransaction = CurrencyAdapter<Balances, DealWithFees<Self>>;
	type TransactionByteFee = TransactionByteFee;
	type OperationalFeeMultiplier = OperationalFeeMultiplier;
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
		pub para_validator: Initializer,
		pub para_assignment: ParaSessionInfo,
		pub authority_discovery: AuthorityDiscovery,
	}
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
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
}

impl pallet_session::historical::Config for Runtime {
	type FullIdentification = pallet_staking::Exposure<AccountId, Balance>;
	type FullIdentificationOf = pallet_staking::ExposureOf<Runtime>;
}

parameter_types! {
	// phase durations. 1/4 of the last session for each.
	// in testing: 1min or half of the session for each
	pub SignedPhase: u32 = prod_or_fast!(
		EPOCH_DURATION_IN_SLOTS / 4,
		(1 * MINUTES).min(EpochDuration::get().saturated_into::<u32>() / 2),
		"KSM_SIGNED_PHASE"
	);
	pub UnsignedPhase: u32 = prod_or_fast!(
		EPOCH_DURATION_IN_SLOTS / 4,
		(1 * MINUTES).min(EpochDuration::get().saturated_into::<u32>() / 2),
		"KSM_UNSIGNED_PHASE"
	);

	// signed config
	pub const SignedMaxSubmissions: u32 = 16;
	pub const SignedDepositBase: Balance = deposit(2, 0);
	pub const SignedDepositByte: Balance = deposit(0, 10) / 1024;
	// Each good submission will get 1/10 KSM as reward
	pub SignedRewardBase: Balance =  UNITS / 10;
	pub SolutionImprovementThreshold: Perbill = Perbill::from_rational(5u32, 10_000);

	// 1 hour session, 15 minutes unsigned phase, 8 offchain executions.
	pub OffchainRepeat: BlockNumber = UnsignedPhase::get() / 8;

	/// Whilst `UseNominatorsAndUpdateBagsList` or `UseNominatorsMap` is in use, this can still be a
	/// very large value. Once the `BagsList` is in full motion, staking might open its door to many
	/// more nominators, and this value should instead be what is a "safe" number (e.g. 22500).
	pub const VoterSnapshotPerBlock: u32 = 22_500;
}

sp_npos_elections::generate_solution_type!(
	#[compact]
	pub struct NposCompactSolution24::<
		VoterIndex = u32,
		TargetIndex = u16,
		Accuracy = sp_runtime::PerU16,
	>(24)
);

impl pallet_election_provider_multi_phase::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type EstimateCallFee = TransactionPayment;
	type UnsignedPhase = UnsignedPhase;
	type SignedMaxSubmissions = SignedMaxSubmissions;
	type SignedRewardBase = SignedRewardBase;
	type SignedDepositBase = SignedDepositBase;
	type SignedDepositByte = SignedDepositByte;
	type SignedDepositWeight = ();
	type SignedMaxWeight = Self::MinerMaxWeight;
	type SlashHandler = (); // burn slashes
	type RewardHandler = (); // nothing to do upon rewards
	type SignedPhase = SignedPhase;
	type SolutionImprovementThreshold = SolutionImprovementThreshold;
	type MinerMaxWeight = OffchainSolutionWeightLimit; // For now use the one from staking.
	type MinerMaxLength = OffchainSolutionLengthLimit;
	type OffchainRepeat = OffchainRepeat;
	type MinerTxPriority = NposSolutionPriority;
	type DataProvider = Staking;
	type Solution = NposCompactSolution24;
	type Fallback = pallet_election_provider_multi_phase::NoFallback<Self>;
	type Solver = frame_election_provider_support::SequentialPhragmen<
		AccountId,
		pallet_election_provider_multi_phase::SolutionAccuracyOf<Self>,
		runtime_common::elections::OffchainRandomBalancing,
	>;
	type BenchmarkingConfig = runtime_common::elections::BenchmarkConfig;
	type ForceOrigin = EnsureOneOf<
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilCollective>,
	>;
	type WeightInfo = weights::pallet_election_provider_multi_phase::WeightInfo<Self>;
	type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
}

parameter_types! {
	pub const BagThresholds: &'static [u64] = &bag_thresholds::THRESHOLDS;
}

impl pallet_bags_list::Config for Runtime {
	type Event = Event;
	type VoteWeightProvider = Staking;
	type WeightInfo = weights::pallet_bags_list::WeightInfo<Runtime>;
	type BagThresholds = BagThresholds;
}

fn era_payout(
	total_staked: Balance,
	non_gilt_issuance: Balance,
	max_annual_inflation: Perquintill,
	period_fraction: Perquintill,
	auctioned_slots: u64,
) -> (Balance, Balance) {
	use pallet_staking_reward_fn::compute_inflation;
	use sp_arithmetic::traits::Saturating;

	let min_annual_inflation = Perquintill::from_rational(25u64, 1000u64);
	let delta_annual_inflation = max_annual_inflation.saturating_sub(min_annual_inflation);

	// 30% reserved for up to 60 slots.
	let auction_proportion = Perquintill::from_rational(auctioned_slots.min(60), 200u64);

	// Therefore the ideal amount at stake (as a percentage of total issuance) is 75% less the amount that we expect
	// to be taken up with auctions.
	let ideal_stake = Perquintill::from_percent(75).saturating_sub(auction_proportion);

	let stake = Perquintill::from_rational(total_staked, non_gilt_issuance);
	let falloff = Perquintill::from_percent(5);
	let adjustment = compute_inflation(stake, ideal_stake, falloff);
	let staking_inflation =
		min_annual_inflation.saturating_add(delta_annual_inflation * adjustment);

	let max_payout = period_fraction * max_annual_inflation * non_gilt_issuance;
	let staking_payout = (period_fraction * staking_inflation) * non_gilt_issuance;
	let rest = max_payout.saturating_sub(staking_payout);

	let other_issuance = non_gilt_issuance.saturating_sub(total_staked);
	if total_staked > other_issuance {
		let _cap_rest = Perquintill::from_rational(other_issuance, total_staked) * staking_payout;
		// We don't do anything with this, but if we wanted to, we could introduce a cap on the treasury amount
		// with: `rest = rest.min(cap_rest);`
	}
	(staking_payout, rest)
}

pub struct EraPayout;
impl pallet_staking::EraPayout<Balance> for EraPayout {
	fn era_payout(
		total_staked: Balance,
		_total_issuance: Balance,
		era_duration_millis: u64,
	) -> (Balance, Balance) {
		// TODO: #3011 Update with proper auctioned slots tracking.
		// This should be fine for the first year of parachains.
		let auctioned_slots: u64 = auctions::Pallet::<Runtime>::auction_counter().into();
		const MAX_ANNUAL_INFLATION: Perquintill = Perquintill::from_percent(10);
		const MILLISECONDS_PER_YEAR: u64 = 1000 * 3600 * 24 * 36525 / 100;

		era_payout(
			total_staked,
			Gilt::issuance().non_gilt,
			MAX_ANNUAL_INFLATION,
			Perquintill::from_rational(era_duration_millis, MILLISECONDS_PER_YEAR),
			auctioned_slots,
		)
	}
}

parameter_types! {
	// Six sessions in an era (6 hours).
	pub const SessionsPerEra: SessionIndex = 6;
	// 28 eras for unbonding (7 days).
	pub const BondingDuration: sp_staking::EraIndex = 28;
	// 27 eras in which slashes can be cancelled (slightly less than 7 days).
	pub const SlashDeferDuration: sp_staking::EraIndex = 27;
	pub const MaxNominatorRewardedPerValidator: u32 = 256;
	pub const OffendingValidatorsThreshold: Perbill = Perbill::from_percent(17);
}

type SlashCancelOrigin = EnsureOneOf<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>,
>;

impl frame_election_provider_support::onchain::Config for Runtime {
	type Accuracy = runtime_common::elections::OnOnChainAccuracy;
	type DataProvider = Staking;
}

impl pallet_staking::Config for Runtime {
	const MAX_NOMINATIONS: u32 =
		<NposCompactSolution24 as sp_npos_elections::NposSolution>::LIMIT as u32;
	type Currency = Balances;
	type UnixTime = Timestamp;
	type CurrencyToVote = CurrencyToVote;
	type ElectionProvider = ElectionProviderMultiPhase;
	type GenesisElectionProvider = runtime_common::elections::GenesisElectionOf<Self>;
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
	type EraPayout = EraPayout;
	type NextNewSession = Session;
	type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
	type OffendingValidatorsThreshold = OffendingValidatorsThreshold;
	type SortedListProvider = BagsList;
	type BenchmarkingConfig = runtime_common::StakingBenchmarkingConfig;
	type WeightInfo = weights::pallet_staking::WeightInfo<Runtime>;
}

parameter_types! {
	pub LaunchPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1, "KSM_LAUNCH_PERIOD");
	pub VotingPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1 * MINUTES, "KSM_VOTING_PERIOD");
	pub FastTrackVotingPeriod: BlockNumber = prod_or_fast!(3 * HOURS, 1 * MINUTES, "KSM_FAST_TRACK_VOTING_PERIOD");
	pub const MinimumDeposit: Balance = 100 * CENTS;
	pub EnactmentPeriod: BlockNumber = prod_or_fast!(8 * DAYS, 1, "KSM_ENACTMENT_PERIOD");
	pub CooloffPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1 * MINUTES, "KSM_COOLOFF_PERIOD");
	pub const InstantAllowed: bool = true;
	pub const MaxVotes: u32 = 100;
	pub const MaxProposals: u32 = 100;
}

impl pallet_democracy::Config for Runtime {
	type Proposal = Call;
	type Event = Event;
	type Currency = Balances;
	type EnactmentPeriod = EnactmentPeriod;
	type VoteLockingPeriod = EnactmentPeriod;
	type LaunchPeriod = LaunchPeriod;
	type VotingPeriod = VotingPeriod;
	type MinimumDeposit = MinimumDeposit;
	/// A straight majority of the council can decide what their next motion is.
	type ExternalOrigin =
		pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>;
	/// A majority can have the next scheduled referendum be a straight majority-carries vote.
	type ExternalMajorityOrigin =
		pallet_collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>;
	/// A unanimous council can have the next scheduled referendum be a straight default-carries
	/// (NTB) vote.
	type ExternalDefaultOrigin =
		pallet_collective::EnsureProportionAtLeast<_1, _1, AccountId, CouncilCollective>;
	/// Two thirds of the technical committee can have an `ExternalMajority/ExternalDefault` vote
	/// be tabled immediately and with a shorter voting/enactment period.
	type FastTrackOrigin =
		pallet_collective::EnsureProportionAtLeast<_2, _3, AccountId, TechnicalCollective>;
	type InstantOrigin =
		pallet_collective::EnsureProportionAtLeast<_1, _1, AccountId, TechnicalCollective>;
	type InstantAllowed = InstantAllowed;
	type FastTrackVotingPeriod = FastTrackVotingPeriod;
	// To cancel a proposal which has been passed, 2/3 of the council must agree to it.
	type CancellationOrigin = EnsureOneOf<
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilCollective>,
	>;
	type BlacklistOrigin = EnsureRoot<AccountId>;
	// To cancel a proposal before it has been passed, the technical committee must be unanimous or
	// Root must agree.
	type CancelProposalOrigin = EnsureOneOf<
		EnsureRoot<AccountId>,
		pallet_collective::EnsureProportionAtLeast<_1, _1, AccountId, TechnicalCollective>,
	>;
	// Any single technical committee member may veto a coming council proposal, however they can
	// only do it once and it lasts only for the cooloff period.
	type VetoOrigin = pallet_collective::EnsureMember<AccountId, TechnicalCollective>;
	type CooloffPeriod = CooloffPeriod;
	type PreimageByteDeposit = PreimageByteDeposit;
	type OperationalPreimageOrigin = pallet_collective::EnsureMember<AccountId, CouncilCollective>;
	type Slash = Treasury;
	type Scheduler = Scheduler;
	type PalletsOrigin = OriginCaller;
	type MaxVotes = MaxVotes;
	type WeightInfo = weights::pallet_democracy::WeightInfo<Runtime>;
	type MaxProposals = MaxProposals;
}

parameter_types! {
	pub CouncilMotionDuration: BlockNumber = prod_or_fast!(3 * DAYS, 2 * MINUTES, "KSM_MOTION_DURATION");
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
	type WeightInfo = weights::pallet_collective_council::WeightInfo<Runtime>;
}

parameter_types! {
	pub const CandidacyBond: Balance = 100 * CENTS;
	// 1 storage item created, key size is 32 bytes, value size is 16+16.
	pub const VotingBondBase: Balance = deposit(1, 64);
	// additional data per vote is 32 bytes (account id).
	pub const VotingBondFactor: Balance = deposit(0, 32);
	/// Daily council elections
	pub TermDuration: BlockNumber = prod_or_fast!(24 * HOURS, 2 * MINUTES, "KSM_TERM_DURATION");
	pub const DesiredMembers: u32 = 19;
	pub const DesiredRunnersUp: u32 = 19;
	pub const PhragmenElectionPalletId: LockIdentifier = *b"phrelect";
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
	type PalletId = PhragmenElectionPalletId;
	type WeightInfo = weights::pallet_elections_phragmen::WeightInfo<Runtime>;
}

parameter_types! {
	pub TechnicalMotionDuration: BlockNumber = prod_or_fast!(3 * DAYS, 2 * MINUTES, "KSM_MOTION_DURATION");
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
	type WeightInfo = weights::pallet_collective_technical_committee::WeightInfo<Runtime>;
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
	pub const BountyDepositBase: Balance = 100 * CENTS;
	pub const BountyDepositPayoutDelay: BlockNumber = 4 * DAYS;
	pub const BountyUpdatePeriod: BlockNumber = 90 * DAYS;
	pub const MaximumReasonLength: u32 = 16384;
	pub const BountyCuratorDeposit: Permill = Permill::from_percent(50);
	pub const BountyValueMinimum: Balance = 200 * CENTS;
	pub const MaxApprovals: u32 = 100;
	pub const MaxAuthorities: u32 = 100_000;
	pub const MaxKeys: u32 = 10_000;
	pub const MaxPeerInHeartbeats: u32 = 10_000;
	pub const MaxPeerDataEncodingSize: u32 = 1_000;
}

type ApproveOrigin = EnsureOneOf<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<_3, _5, AccountId, CouncilCollective>,
>;

impl pallet_treasury::Config for Runtime {
	type PalletId = TreasuryPalletId;
	type Currency = Balances;
	type ApproveOrigin = ApproveOrigin;
	type RejectOrigin = MoreThanHalfCouncil;
	type Event = Event;
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
}

impl pallet_bounties::Config for Runtime {
	type BountyDepositBase = BountyDepositBase;
	type BountyDepositPayoutDelay = BountyDepositPayoutDelay;
	type BountyUpdatePeriod = BountyUpdatePeriod;
	type BountyCuratorDeposit = BountyCuratorDeposit;
	type BountyValueMinimum = BountyValueMinimum;
	type ChildBountyManager = ();
	type DataDepositPerByte = DataDepositPerByte;
	type Event = Event;
	type MaximumReasonLength = MaximumReasonLength;
	type WeightInfo = weights::pallet_bounties::WeightInfo<Runtime>;
}

impl pallet_tips::Config for Runtime {
	type MaximumReasonLength = MaximumReasonLength;
	type DataDepositPerByte = DataDepositPerByte;
	type Tippers = PhragmenElection;
	type TipCountdown = TipCountdown;
	type TipFindersFee = TipFindersFee;
	type TipReportDepositBase = TipReportDepositBase;
	type Event = Event;
	type WeightInfo = weights::pallet_tips::WeightInfo<Runtime>;
}

impl pallet_offences::Config for Runtime {
	type Event = Event;
	type IdentificationTuple = pallet_session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = Staking;
}

impl pallet_authority_discovery::Config for Runtime {
	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub NposSolutionPriority: TransactionPriority =
		Perbill::from_percent(90) * TransactionPriority::max_value();
	pub const ImOnlineUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl pallet_im_online::Config for Runtime {
	type AuthorityId = ImOnlineId;
	type Event = Event;
	type ValidatorSet = Historical;
	type NextSessionRotation = Babe;
	type ReportUnresponsiveness = Offences;
	type UnsignedPriority = ImOnlineUnsignedPriority;
	type WeightInfo = weights::pallet_im_online::WeightInfo<Runtime>;
	type MaxKeys = MaxKeys;
	type MaxPeerInHeartbeats = MaxPeerInHeartbeats;
	type MaxPeerDataEncodingSize = MaxPeerDataEncodingSize;
}

impl pallet_grandpa::Config for Runtime {
	type Event = Event;
	type Call = Call;

	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		GrandpaId,
	)>>::IdentificationTuple;

	type KeyOwnerProofSystem = Historical;

	type HandleEquivocation = pallet_grandpa::EquivocationHandler<
		Self::KeyOwnerIdentification,
		Offences,
		ReportLongevity,
	>;

	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
}

/// Submits transaction with the node's public and signature type. Adheres to the signed extension
/// format of the chain.
impl<LocalCall> frame_system::offchain::CreateSignedTransaction<LocalCall> for Runtime
where
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
	Call: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = Call;
}

parameter_types! {
	pub Prefix: &'static [u8] = b"Pay KSMs to the Kusama account:";
}

impl claims::Config for Runtime {
	type Event = Event;
	type VestingSchedule = Vesting;
	type Prefix = Prefix;
	type MoveClaimOrigin =
		pallet_collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type WeightInfo = weights::runtime_common_claims::WeightInfo<Runtime>;
}

parameter_types! {
	// Minimum 100 bytes/KSM deposited (1 CENT/byte)
	pub const BasicDeposit: Balance = 1000 * CENTS;       // 258 bytes on-chain
	pub const FieldDeposit: Balance = 250 * CENTS;        // 66 bytes on-chain
	pub const SubAccountDeposit: Balance = 200 * CENTS;   // 53 bytes on-chain
	pub const MaxSubAccounts: u32 = 100;
	pub const MaxAdditionalFields: u32 = 100;
	pub const MaxRegistrars: u32 = 20;
}

impl pallet_identity::Config for Runtime {
	type Event = Event;
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
	type Event = Event;
	type Call = Call;
	type PalletsOrigin = OriginCaller;
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
	pub const ConfigDepositBase: Balance = 500 * CENTS;
	pub const FriendDepositFactor: Balance = 50 * CENTS;
	pub const MaxFriends: u16 = 9;
	pub const RecoveryDeposit: Balance = 500 * CENTS;
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
	pub const CandidateDeposit: Balance = 1000 * CENTS;
	pub const WrongSideDeduction: Balance = 200 * CENTS;
	pub const MaxStrikes: u32 = 10;
	pub const RotationPeriod: BlockNumber = 7 * DAYS;
	pub const PeriodSpend: Balance = 50000 * CENTS;
	pub const MaxLockDuration: BlockNumber = 36 * 30 * DAYS;
	pub const ChallengePeriod: BlockNumber = 7 * DAYS;
	pub const MaxCandidateIntake: u32 = 1;
	pub const SocietyPalletId: PalletId = PalletId(*b"py/socie");
}

impl pallet_society::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type CandidateDeposit = CandidateDeposit;
	type WrongSideDeduction = WrongSideDeduction;
	type MaxStrikes = MaxStrikes;
	type PeriodSpend = PeriodSpend;
	type MembershipChanged = ();
	type RotationPeriod = RotationPeriod;
	type MaxLockDuration = MaxLockDuration;
	type FounderSetOrigin =
		pallet_collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type SuspensionJudgementOrigin = pallet_society::EnsureFounder<Runtime>;
	type ChallengePeriod = ChallengePeriod;
	type MaxCandidateIntake = MaxCandidateIntake;
	type PalletId = SocietyPalletId;
}

parameter_types! {
	pub const MinVestedTransfer: Balance = 100 * CENTS;
}

impl pallet_vesting::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type BlockNumberToBalance = ConvertInto;
	type MinVestedTransfer = MinVestedTransfer;
	type WeightInfo = weights::pallet_vesting::WeightInfo<Runtime>;
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
	scale_info::TypeInfo,
)]
pub enum ProxyType {
	Any,
	NonTransfer,
	Governance,
	Staking,
	IdentityJudgement,
	CancelProxy,
	Auction,
}
impl Default for ProxyType {
	fn default() -> Self {
		Self::Any
	}
}
impl InstanceFilter<Call> for ProxyType {
	fn filter(&self, c: &Call) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::NonTransfer => matches!(
				c,
				Call::System(..) |
				Call::Babe(..) |
				Call::Timestamp(..) |
				Call::Indices(pallet_indices::Call::claim {..}) |
				Call::Indices(pallet_indices::Call::free {..}) |
				Call::Indices(pallet_indices::Call::freeze {..}) |
				// Specifically omitting Indices `transfer`, `force_transfer`
				// Specifically omitting the entire Balances pallet
				Call::Authorship(..) |
				Call::Staking(..) |
				Call::Session(..) |
				Call::Grandpa(..) |
				Call::ImOnline(..) |
				Call::Democracy(..) |
				Call::Council(..) |
				Call::TechnicalCommittee(..) |
				Call::PhragmenElection(..) |
				Call::TechnicalMembership(..) |
				Call::Treasury(..) |
				Call::Bounties(..) |
				Call::Tips(..) |
				Call::Claims(..) |
				Call::Utility(..) |
				Call::Identity(..) |
				Call::Society(..) |
				Call::Recovery(pallet_recovery::Call::as_recovered {..}) |
				Call::Recovery(pallet_recovery::Call::vouch_recovery {..}) |
				Call::Recovery(pallet_recovery::Call::claim_recovery {..}) |
				Call::Recovery(pallet_recovery::Call::close_recovery {..}) |
				Call::Recovery(pallet_recovery::Call::remove_recovery {..}) |
				Call::Recovery(pallet_recovery::Call::cancel_recovered {..}) |
				// Specifically omitting Recovery `create_recovery`, `initiate_recovery`
				Call::Vesting(pallet_vesting::Call::vest {..}) |
				Call::Vesting(pallet_vesting::Call::vest_other {..}) |
				// Specifically omitting Vesting `vested_transfer`, and `force_vested_transfer`
				Call::Scheduler(..) |
				Call::Proxy(..) |
				Call::Multisig(..) |
				Call::Gilt(..) |
				Call::Registrar(paras_registrar::Call::register {..}) |
				Call::Registrar(paras_registrar::Call::deregister {..}) |
				// Specifically omitting Registrar `swap`
				Call::Registrar(paras_registrar::Call::reserve {..}) |
				Call::Crowdloan(..) |
				Call::Slots(..) |
				Call::Auctions(..) | // Specifically omitting the entire XCM Pallet
				Call::BagsList(..)
			),
			ProxyType::Governance => matches!(
				c,
				Call::Democracy(..) |
					Call::Council(..) | Call::TechnicalCommittee(..) |
					Call::PhragmenElection(..) |
					Call::Treasury(..) | Call::Bounties(..) |
					Call::Tips(..) | Call::Utility(..)
			),
			ProxyType::Staking => {
				matches!(c, Call::Staking(..) | Call::Session(..) | Call::Utility(..))
			},
			ProxyType::IdentityJudgement => matches!(
				c,
				Call::Identity(pallet_identity::Call::provide_judgement { .. }) | Call::Utility(..)
			),
			ProxyType::CancelProxy => {
				matches!(c, Call::Proxy(pallet_proxy::Call::reject_announcement { .. }))
			},
			ProxyType::Auction => matches!(
				c,
				Call::Auctions(..) | Call::Crowdloan(..) | Call::Registrar(..) | Call::Slots(..)
			),
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

impl parachains_origin::Config for Runtime {}

impl parachains_configuration::Config for Runtime {
	type WeightInfo = weights::runtime_parachains_configuration::WeightInfo<Runtime>;
}

impl parachains_shared::Config for Runtime {}

impl parachains_session_info::Config for Runtime {}

impl parachains_inclusion::Config for Runtime {
	type Event = Event;
	type DisputesHandler = ParasDisputes;
	type RewardValidators = parachains_reward_points::RewardValidatorsWithEraPoints<Runtime>;
}

parameter_types! {
	pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl parachains_paras::Config for Runtime {
	type Event = Event;
	type WeightInfo = weights::runtime_parachains_paras::WeightInfo<Runtime>;
	type UnsignedPriority = ParasUnsignedPriority;
	type NextSessionRotation = Babe;
}

parameter_types! {
	pub const FirstMessageFactorPercent: u64 = 100;
}

impl parachains_ump::Config for Runtime {
	type Event = Event;
	type UmpSink =
		crate::parachains_ump::XcmSink<xcm_executor::XcmExecutor<xcm_config::XcmConfig>, Runtime>;
	type FirstMessageFactorPercent = FirstMessageFactorPercent;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
}

impl parachains_dmp::Config for Runtime {}

impl parachains_hrmp::Config for Runtime {
	type Event = Event;
	type Origin = Origin;
	type Currency = Balances;
	type WeightInfo = weights::runtime_parachains_hrmp::WeightInfo<Self>;
}

impl parachains_paras_inherent::Config for Runtime {
	type WeightInfo = weights::runtime_parachains_paras_inherent::WeightInfo<Runtime>;
}

impl parachains_scheduler::Config for Runtime {}

impl parachains_initializer::Config for Runtime {
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type ForceOrigin = EnsureRoot<AccountId>;
	type WeightInfo = weights::runtime_parachains_initializer::WeightInfo<Runtime>;
}

impl parachains_disputes::Config for Runtime {
	type Event = Event;
	type RewardValidators = ();
	type PunishValidators = ();
	type WeightInfo = weights::runtime_parachains_disputes::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ParaDeposit: Balance = 40 * UNITS;
}

impl paras_registrar::Config for Runtime {
	type Event = Event;
	type Origin = Origin;
	type Currency = Balances;
	type OnSwap = (Crowdloan, Slots);
	type ParaDeposit = ParaDeposit;
	type DataDepositPerByte = DataDepositPerByte;
	type WeightInfo = weights::runtime_common_paras_registrar::WeightInfo<Runtime>;
}

parameter_types! {
	// 6 weeks
	pub LeasePeriod: BlockNumber = prod_or_fast!(6 * WEEKS, 6 * WEEKS, "KSM_LEASE_PERIOD");
}

impl slots::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Registrar = Registrar;
	type LeasePeriod = LeasePeriod;
	type LeaseOffset = ();
	type ForceOrigin = MoreThanHalfCouncil;
	type WeightInfo = weights::runtime_common_slots::WeightInfo<Runtime>;
}

parameter_types! {
	pub const CrowdloanId: PalletId = PalletId(*b"py/cfund");
	pub const SubmissionDeposit: Balance = 3 * GRAND; // ~ 10 KSM
	pub const MinContribution: Balance = 3_000 * CENTS; // ~ .1 KSM
	pub const RemoveKeysLimit: u32 = 1000;
	// Allow 32 bytes for an additional memo to a crowdloan.
	pub const MaxMemoLength: u8 = 32;
}

impl crowdloan::Config for Runtime {
	type Event = Event;
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

type AuctionInitiate = EnsureOneOf<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilCollective>,
>;

impl auctions::Config for Runtime {
	type Event = Event;
	type Leaser = Slots;
	type Registrar = Registrar;
	type EndingPeriod = EndingPeriod;
	type SampleLength = SampleLength;
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type InitiateOrigin = AuctionInitiate;
	type WeightInfo = weights::runtime_common_auctions::WeightInfo<Runtime>;
}

parameter_types! {
	pub IgnoredIssuance: Balance = Treasury::pot();
	pub const QueueCount: u32 = 300;
	pub const MaxQueueLen: u32 = 1000;
	pub const FifoQueueLen: u32 = 250;
	pub const GiltPeriod: BlockNumber = 30 * DAYS;
	pub const MinFreeze: Balance = 10_000 * CENTS;
	pub const IntakePeriod: BlockNumber = 5 * MINUTES;
	pub const MaxIntakeBids: u32 = 100;
}

impl pallet_gilt::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type CurrencyBalance = Balance;
	type AdminOrigin = MoreThanHalfCouncil;
	type Deficit = (); // Mint
	type Surplus = (); // Burn
	type IgnoredIssuance = IgnoredIssuance;
	type QueueCount = QueueCount;
	type MaxQueueLen = MaxQueueLen;
	type FifoQueueLen = FifoQueueLen;
	type Period = GiltPeriod;
	type MinFreeze = MinFreeze;
	type IntakePeriod = IntakePeriod;
	type MaxIntakeBids = MaxIntakeBids;
	type WeightInfo = weights::pallet_gilt::WeightInfo<Runtime>;
}

construct_runtime! {
	pub enum Runtime where
		Block = Block,
		NodeBlock = primitives::v1::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		// Basic stuff; balances is uncallable initially.
		System: frame_system::{Pallet, Call, Storage, Config, Event<T>} = 0,

		// Babe must be before session.
		Babe: pallet_babe::{Pallet, Call, Storage, Config, ValidateUnsigned} = 1,

		Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent} = 2,
		Indices: pallet_indices::{Pallet, Call, Storage, Config<T>, Event<T>} = 3,
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>} = 4,
		TransactionPayment: pallet_transaction_payment::{Pallet, Storage} = 33,

		// Consensus support.
		// Authorship must be before session in order to note author in the correct session and era
		// for im-online and staking.
		Authorship: pallet_authorship::{Pallet, Call, Storage} = 5,
		Staking: pallet_staking::{Pallet, Call, Storage, Config<T>, Event<T>} = 6,
		Offences: pallet_offences::{Pallet, Storage, Event} = 7,
		Historical: session_historical::{Pallet} = 34,
		Session: pallet_session::{Pallet, Call, Storage, Event, Config<T>} = 8,
		Grandpa: pallet_grandpa::{Pallet, Call, Storage, Config, Event, ValidateUnsigned} = 10,
		ImOnline: pallet_im_online::{Pallet, Call, Storage, Event<T>, ValidateUnsigned, Config<T>} = 11,
		AuthorityDiscovery: pallet_authority_discovery::{Pallet, Config} = 12,

		// Governance stuff; uncallable initially.
		Democracy: pallet_democracy::{Pallet, Call, Storage, Config<T>, Event<T>} = 13,
		Council: pallet_collective::<Instance1>::{Pallet, Call, Storage, Origin<T>, Event<T>, Config<T>} = 14,
		TechnicalCommittee: pallet_collective::<Instance2>::{Pallet, Call, Storage, Origin<T>, Event<T>, Config<T>} = 15,
		PhragmenElection: pallet_elections_phragmen::{Pallet, Call, Storage, Event<T>, Config<T>} = 16,
		TechnicalMembership: pallet_membership::<Instance1>::{Pallet, Call, Storage, Event<T>, Config<T>} = 17,
		Treasury: pallet_treasury::{Pallet, Call, Storage, Config, Event<T>} = 18,


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

		// Bounties module.
		Bounties: pallet_bounties::{Pallet, Call, Storage, Event<T>} = 35,

		// Tips module.
		Tips: pallet_tips::{Pallet, Call, Storage, Event<T>} = 36,

		// Election pallet. Only works with staking, but placed here to maintain indices.
		ElectionProviderMultiPhase: pallet_election_provider_multi_phase::{Pallet, Call, Storage, Event<T>, ValidateUnsigned} = 37,

		// Gilts pallet.
		Gilt: pallet_gilt::{Pallet, Call, Storage, Event<T>, Config} = 38,

		// Provides a semi-sorted list of nominators for staking.
		BagsList: pallet_bags_list::{Pallet, Call, Storage, Event<T>} = 39,

		// Parachains pallets. Start indices at 50 to leave room.
		ParachainsOrigin: parachains_origin::{Pallet, Origin} = 50,
		Configuration: parachains_configuration::{Pallet, Call, Storage, Config<T>} = 51,
		ParasShared: parachains_shared::{Pallet, Call, Storage} = 52,
		ParaInclusion: parachains_inclusion::{Pallet, Call, Storage, Event<T>} = 53,
		ParaInherent: parachains_paras_inherent::{Pallet, Call, Storage, Inherent} = 54,
		ParaScheduler: parachains_scheduler::{Pallet, Storage} = 55,
		Paras: parachains_paras::{Pallet, Call, Storage, Event, Config} = 56,
		Initializer: parachains_initializer::{Pallet, Call, Storage} = 57,
		Dmp: parachains_dmp::{Pallet, Call, Storage} = 58,
		Ump: parachains_ump::{Pallet, Call, Storage, Event} = 59,
		Hrmp: parachains_hrmp::{Pallet, Call, Storage, Event<T>, Config} = 60,
		ParaSessionInfo: parachains_session_info::{Pallet, Storage} = 61,
		ParasDisputes: parachains_disputes::{Pallet, Call, Storage, Event<T>} = 62,

		// Parachain Onboarding Pallets. Start indices at 70 to leave room.
		Registrar: paras_registrar::{Pallet, Call, Storage, Event<T>} = 70,
		Slots: slots::{Pallet, Call, Storage, Event<T>} = 71,
		Auctions: auctions::{Pallet, Call, Storage, Event<T>} = 72,
		Crowdloan: crowdloan::{Pallet, Call, Storage, Event<T>} = 73,

		// Pallet for sending XCM.
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>, Origin, Config} = 99,
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
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<Address, Call, Signature, SignedExtra>;
/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
	(SchedulerMigrationV3, RefundNickPalletDeposit),
>;
/// The payload being signed in the transactions.
pub type SignedPayload = generic::SignedPayload<Call, SignedExtra>;

/// The nicks pallet was removed at block 569325 from the runtime, without any consideration of the
/// fact that numerous accounts had reserved funds in this pallet. This migration is the outcome of
/// an investigation that tries to refund all of the accounts that had a nick set for them prior to
/// removal, if they still have the amount in their reserved balance. Otherwise, we ignore the
/// refund.
pub struct RefundNickPalletDeposit;

impl RefundNickPalletDeposit {
	fn execute(check: bool) -> Weight {
		let accounts_and_deposits = vec![
			(
				// FDDy3cQa7JXiChYU2xq1B2WUUJBpZpZ51qn2tiN1DqDMEpS
				[
					116, 157, 220, 147, 166, 93, 254, 195, 175, 39, 204, 116, 120, 33, 44, 183,
					212, 176, 192, 53, 127, 239, 53, 160, 22, 57, 102, 171, 83, 51, 183, 87,
				],
				10000000000,
			),
			(
				// G7Ur4BnMSfP2qE7ruSob5gwGQ5nzkGWu7Yqh14FcMqnDtgB
				[
					156, 120, 182, 33, 219, 174, 128, 170, 103, 151, 162, 143, 117, 32, 89, 238,
					241, 171, 215, 99, 218, 189, 163, 89, 85, 96, 160, 52, 143, 248, 46, 57,
				],
				10000000000,
			),
			(
				// Ft8UrgCxMoChGP4o7Q92PQqEHaF9UVZQXVW1wbMV7Cf6ost
				[
					146, 73, 247, 0, 26, 190, 182, 82, 240, 43, 224, 199, 223, 167, 173, 151, 130,
					188, 113, 208, 86, 81, 255, 20, 235, 214, 89, 225, 229, 159, 130, 126,
				],
				10000000000000,
			),
			(
				// FFdDXFK1VKG5QgjvqwxdVjo8hGrBveaBFfHnWyz1MAmLL82
				[
					118, 114, 158, 23, 173, 49, 70, 157, 235, 203, 96, 243, 206, 54, 34, 247, 145,
					67, 228, 66, 231, 123, 88, 214, 226, 25, 93, 158, 169, 152, 104, 13,
				],
				10000000000,
			),
			(
				// HqE13RoY1yntxvAvySn8ogit5XrX1EAxZe4HPPaFf48q8JM
				[
					232, 139, 71, 107, 182, 41, 146, 230, 64, 3, 205, 166, 216, 146, 173, 149, 225,
					180, 93, 128, 227, 254, 240, 29, 10, 65, 25, 225, 235, 227, 163, 6,
				],
				10000000000,
			),
			(
				// GLiebiQp5f6G5vNcc7BgRE9T3hrZSYDwP6evERn3hEczdaM
				[
					166, 145, 93, 111, 179, 12, 211, 3, 103, 242, 49, 148, 198, 136, 66, 166, 1,
					143, 86, 92, 119, 62, 160, 197, 68, 235, 42, 98, 89, 123, 27, 52,
				],
				10000000000000,
			),
			(
				// H2LxnKzFLh1KZKjVAGGtpn9R5KSCPuz4ef8HE2Y6BirmV9J
				[
					196, 202, 45, 138, 99, 252, 222, 80, 235, 39, 124, 225, 161, 124, 245, 58, 201,
					169, 220, 211, 178, 228, 57, 232, 95, 139, 4, 107, 208, 220, 253, 4,
				],
				10000000000,
			),
			(
				// Ewo4ddXr9qDEBwCwd5SfqLEPYLJLgAC5vgFhKLWhpRg7MQJ
				[
					104, 217, 88, 232, 193, 243, 108, 203, 24, 225, 126, 17, 81, 187, 250, 67, 25,
					203, 79, 181, 120, 177, 79, 11, 116, 184, 178, 140, 178, 213, 240, 31,
				],
				10000000000,
			),
			(
				// CanLB42xJughpTRC1vXStUryjWYkE679emign1af47QnAQC
				[
					0, 90, 67, 60, 142, 21, 28, 129, 174, 148, 133, 68, 244, 203, 7, 98, 43, 24,
					168, 67, 4, 128, 222, 111, 198, 225, 163, 139, 196, 111, 156, 39,
				],
				10000000000,
			),
			(
				// GPcwRJAb8VXPX452qEGaag3QYDwJLHkGijDrtc1TeBH3FHf
				[
					168, 199, 225, 48, 144, 182, 67, 197, 174, 85, 150, 8, 96, 63, 67, 253, 7, 128,
					178, 243, 213, 59, 77, 181, 81, 101, 86, 29, 34, 92, 124, 45,
				],
				10000000000,
			),
			(
				// GAfhRsNqB9hwEmRFPhMZCvicFJ1kVtRF3UChYyKHq612ZV9
				[
					158, 230, 249, 239, 92, 180, 210, 75, 53, 25, 199, 239, 202, 49, 166, 13, 194,
					124, 33, 81, 0, 1, 78, 80, 248, 135, 31, 235, 56, 26, 235, 186,
				],
				10000000000,
			),
			(
				// FhmaBgRFjZYFUWJp91ZHUbuW2Dm32VH4BVhnRQJqfpMmEok
				[
					138, 99, 32, 177, 239, 122, 152, 201, 19, 158, 75, 250, 108, 96, 161, 227, 10,
					115, 59, 39, 247, 217, 221, 83, 214, 72, 194, 124, 225, 114, 108, 56,
				],
				10000000000,
			),
			(
				// Gc6YgfbTJ6pYXcwib6mk3KoiwncLm4dfdcN3nwFjvfi4Agd
				[
					178, 75, 194, 233, 30, 119, 193, 28, 8, 253, 181, 124, 87, 91, 64, 84, 177,
					159, 216, 231, 126, 12, 246, 200, 159, 231, 169, 131, 127, 242, 143, 80,
				],
				10000000000,
			),
			(
				// GffdZaTf5oBcHVfx4VjnA6AMpHEBa3PiaBzTBUPkt24fUxp
				[
					181, 4, 222, 91, 235, 149, 207, 1, 80, 211, 109, 110, 232, 187, 181, 160, 24,
					78, 166, 154, 54, 198, 207, 12, 252, 44, 151, 122, 57, 70, 214, 176,
				],
				10000000000,
			),
			(
				// DrkY92Yq67zJ7T8XWN7SAXbnNDJhzkADutwhhhPyW3tQKTw
				[
					56, 196, 98, 104, 248, 246, 247, 221, 132, 4, 28, 243, 187, 250, 188, 3, 109,
					117, 244, 101, 120, 147, 102, 217, 21, 24, 194, 80, 176, 96, 112, 24,
				],
				10000000000,
			),
			(
				// CcKPhXSyZgATZD1wVaRsSk81UfLcQvyuuS2i9FNhsoeQeWr
				[
					1, 134, 17, 101, 243, 129, 19, 73, 139, 78, 95, 178, 240, 204, 18, 192, 163,
					111, 29, 159, 254, 26, 30, 39, 13, 35, 69, 169, 55, 81, 143, 194,
				],
				10000000000,
			),
			(
				// HMELERjKFft5puf4xvg9aPADqtpvzwxK3zwbz9cWC9fX3ja
				[
					211, 49, 133, 124, 26, 82, 99, 44, 214, 26, 99, 190, 104, 11, 183, 87, 191,
					209, 240, 131, 56, 101, 64, 111, 117, 30, 16, 150, 203, 224, 42, 124,
				],
				10000000000,
			),
			(
				// E58yuhUAwWzhn2V4thF3VciAJU75eePPipMhxWZe9JKVVfq
				[
					66, 54, 226, 49, 17, 165, 160, 250, 25, 161, 182, 182, 242, 59, 234, 70, 243,
					22, 193, 31, 221, 235, 209, 13, 97, 189, 96, 183, 169, 66, 196, 94,
				],
				10000000000,
			),
			(
				// Dab4bfYTZRUDMWjYAUQuFbDreQ9mt7nULWu3Dw7jodbzVe9
				[
					44, 112, 144, 18, 248, 7, 175, 143, 195, 240, 210, 171, 176, 197, 28, 169, 168,
					141, 78, 242, 77, 26, 9, 43, 248, 157, 172, 245, 206, 99, 234, 29,
				],
				10000000000,
			),
			(
				// H2QeEejU61uwHSXZPvWb4csGdJGxetcViN3eSkTxgbHa4et
				[
					196, 214, 145, 18, 0, 141, 73, 140, 244, 113, 116, 30, 62, 23, 129, 202, 136,
					189, 254, 234, 4, 99, 219, 98, 117, 43, 180, 149, 31, 45, 166, 42,
				],
				10000000000,
			),
			(
				// DbF59HrqrrPh9L2Fi4EBd7gn4xFUSXmrE6zyMzf3pETXLvg
				[
					44, 240, 131, 139, 5, 251, 24, 39, 24, 222, 133, 149, 37, 250, 30, 109, 83,
					213, 87, 229, 252, 246, 49, 238, 159, 244, 76, 97, 152, 16, 212, 59,
				],
				10000000000,
			),
			(
				// Em9FTrdPbJRKRrNNteu3NeR98ZJQ7yQmFqxNeafftGnkdZE
				[
					96, 185, 154, 62, 31, 238, 141, 116, 79, 8, 41, 10, 42, 222, 97, 178, 110, 4,
					72, 98, 239, 183, 202, 96, 220, 12, 148, 78, 249, 217, 170, 122,
				],
				10000000000000,
			),
			(
				// Gi1kynZMWWcQMrvv5wL1TcGkbqT9R4N7fMibSiphSDH7HJq
				[
					182, 207, 29, 171, 210, 158, 113, 56, 63, 232, 79, 168, 37, 141, 241, 134, 175,
					126, 224, 152, 119, 99, 164, 118, 38, 81, 33, 204, 1, 140, 4, 39,
				],
				10000000000,
			),
			(
				// HWAGAxX2PAzNVg7w3ZyTprH5yvwbVwQ8rbWwuZxtQKbQupW
				[
					218, 1, 7, 123, 220, 2, 95, 215, 121, 204, 33, 201, 118, 7, 39, 236, 7, 229,
					42, 161, 50, 65, 11, 130, 229, 250, 186, 203, 111, 69, 176, 85,
				],
				10000000000000,
			),
			(
				// DQSRrYtjyikVMNHkdyXWkhimCDHenKqS4FJYk9r781LSFqA
				[
					36, 179, 11, 183, 207, 19, 243, 240, 216, 103, 254, 154, 12, 154, 85, 139, 97,
					128, 92, 13, 220, 178, 241, 104, 6, 214, 139, 218, 162, 142, 92, 126,
				],
				10000000000,
			),
			(
				// Gth5jQA6v9EFbpqSPgXcsvpGSrbTdWwmBADnqa36ptjs5m5
				[
					190, 243, 241, 170, 113, 179, 43, 186, 119, 91, 56, 134, 185, 0, 162, 227, 251,
					79, 65, 99, 213, 140, 27, 206, 10, 174, 207, 224, 181, 92, 27, 95,
				],
				10000000000,
			),
			(
				// DMELETGdziwQNCqLRR7JKfgYD3Xahnoxi9n7gUHUmuxwGUN
				[
					34, 64, 150, 111, 139, 218, 171, 2, 162, 146, 155, 46, 84, 108, 118, 149, 171,
					221, 184, 116, 228, 69, 194, 98, 68, 147, 137, 9, 198, 27, 176, 66,
				],
				10000000000,
			),
			(
				// EicrAEbyauqktQpp4CdvsF2CQy3Ju7tGGMohj3h5sAPnKHL
				[
					94, 204, 197, 40, 211, 144, 196, 33, 206, 39, 175, 184, 65, 108, 201, 54, 243,
					173, 85, 144, 42, 182, 41, 127, 37, 114, 167, 248, 149, 164, 177, 53,
				],
				10000000000,
			),
			(
				// GaVMNNUSnBsEuuYyQXbEEmWiSuTSDyRWgfDKTSKagpzteim
				[
					177, 18, 9, 233, 120, 11, 2, 165, 55, 66, 10, 134, 94, 109, 23, 133, 52, 212,
					151, 67, 79, 143, 238, 215, 36, 124, 107, 222, 202, 23, 151, 86,
				],
				10000000000,
			),
			(
				// CsM49wAGnAUo9RUCEZYsyBYtWUKKDT9hvH7fFiqRHeHC1op
				[
					12, 252, 88, 230, 28, 182, 44, 37, 252, 63, 69, 165, 96, 5, 247, 182, 206, 194,
					47, 232, 78, 237, 106, 199, 200, 153, 44, 227, 32, 154, 176, 36,
				],
				10000000000,
			),
			(
				// FAGzHVggwv1QRmkGjom1Foc24jzZS1CJGcWUzrGdW8FyXEm
				[
					114, 94, 75, 214, 238, 141, 247, 28, 214, 219, 162, 225, 46, 51, 173, 190, 96,
					181, 41, 240, 0, 11, 168, 158, 115, 135, 144, 32, 185, 110, 151, 142,
				],
				10000000000,
			),
			(
				// FAYyBS6arn3X4fvtdybaBUdw5zqVsv3PPwRXXXYRTzTGFDv
				[
					114, 148, 23, 203, 43, 181, 231, 49, 225, 161, 203, 113, 102, 89, 218, 193,
					205, 54, 234, 214, 49, 118, 110, 212, 147, 239, 120, 136, 227, 131, 148, 47,
				],
				10000000000,
			),
			(
				// HWpid7FWbuA7GxN1yykrYqSe6cY3ybUaX3qErCoiiFDSqSh
				[
					218, 130, 123, 220, 174, 27, 214, 107, 33, 76, 0, 232, 131, 190, 29, 95, 29,
					117, 67, 1, 172, 212, 188, 13, 174, 147, 197, 98, 216, 92, 250, 14,
				],
				10000000000,
			),
			(
				// EPehck28w8fjRZCqZ3VZA7XpwGeipuVU4ETSwv7GkF5BdcG
				[
					80, 85, 88, 32, 26, 242, 233, 133, 139, 76, 38, 98, 125, 118, 2, 38, 102, 115,
					182, 64, 117, 231, 219, 127, 220, 111, 71, 158, 96, 54, 1, 89,
				],
				10000000000,
			),
			(
				// GJ7JBaqx4ys5LrkECQKmEWdjGgAoXChsLepjEqdHuKggymK
				[
					164, 147, 222, 101, 93, 106, 87, 193, 54, 226, 40, 40, 244, 107, 133, 50, 226,
					211, 28, 198, 242, 231, 165, 199, 186, 42, 32, 230, 137, 247, 84, 12,
				],
				10000000000,
			),
			(
				// HLLfrFFvmVtjsKY4GjjUz3AEKXAhgrWXuXZBtQKgwXZ1kMP
				[
					210, 131, 156, 199, 115, 228, 244, 83, 96, 154, 217, 103, 175, 17, 75, 14, 66,
					224, 223, 195, 68, 225, 127, 149, 37, 12, 131, 145, 38, 83, 106, 91,
				],
				10000000000,
			),
			(
				// EJLzRAmQmqHuTdVKFWTUmA2DZHC44C1nVwnEzmd5zoAnTUx
				[
					76, 73, 127, 184, 138, 155, 209, 12, 223, 194, 107, 132, 238, 248, 247, 173,
					215, 128, 71, 244, 15, 76, 59, 10, 185, 14, 138, 29, 107, 106, 185, 69,
				],
				10000000000,
			),
			(
				// GZ5xCKCC8JRpJSMgAoDJAFAuQV8gD9Xz2cJca4mR7oRhzL9
				[
					176, 0, 5, 255, 248, 249, 234, 120, 54, 98, 11, 215, 14, 115, 67, 174, 174, 54,
					227, 157, 128, 171, 217, 69, 20, 210, 10, 41, 26, 91, 115, 90,
				],
				10000000000,
			),
			(
				// DydhvUrMzhmvsbtHxCY5PGkWwRP3bRqcnFcwBJ1StZdgpXs
				[
					62, 4, 27, 33, 225, 137, 152, 173, 113, 95, 10, 12, 46, 9, 217, 113, 110, 130,
					72, 166, 241, 50, 146, 87, 245, 127, 114, 38, 95, 98, 38, 13,
				],
				10000000000,
			),
			(
				// GstVeDt8rAUm35e15Voytmo4z3wJu4aFGYDhbcrEubgpBLS
				[
					190, 87, 29, 156, 216, 198, 240, 163, 43, 82, 176, 28, 234, 108, 82, 70, 52,
					108, 167, 243, 92, 5, 119, 40, 141, 205, 123, 189, 144, 21, 143, 42,
				],
				10000000000,
			),
			(
				// H72hS8xLmSiSBqbBXHND2KbN8PAoevi52B685cbGki6T9nt
				[
					200, 92, 235, 179, 242, 27, 94, 151, 115, 122, 123, 188, 20, 216, 55, 106, 121,
					219, 186, 156, 40, 73, 162, 142, 218, 231, 124, 119, 109, 30, 74, 8,
				],
				10000000000000,
			),
			(
				// JHTXKwYBhRZKWPW3nbtwtfE8dfRDYCS65cJXueMhg11k4cs
				[
					252, 141, 45, 218, 128, 204, 60, 241, 174, 68, 89, 62, 144, 165, 22, 79, 237,
					228, 166, 254, 38, 15, 56, 180, 249, 121, 233, 120, 251, 24, 29, 26,
				],
				10000000000,
			),
			(
				// DSpbbk6HKKyS78c4KDLSxCetqbwnsemv2iocVXwNe2FAvWC
				[
					38, 132, 41, 39, 201, 138, 80, 171, 29, 67, 154, 180, 95, 33, 197, 190, 182,
					151, 5, 86, 225, 253, 123, 82, 223, 68, 151, 126, 67, 68, 177, 72,
				],
				10000000000,
			),
			(
				// EZ7uBY7ZLohavWAugjTSUVVSABLfad77S6RQf4pDe3cV9q4
				[
					87, 142, 29, 62, 121, 108, 29, 242, 4, 206, 176, 221, 8, 65, 178, 141, 245,
					156, 168, 27, 30, 73, 224, 200, 30, 8, 218, 17, 235, 8, 129, 88,
				],
				10000000000,
			),
			(
				// GTzRQPzkcuynHgkEHhsPBFpKdh4sAacVRsnd8vYfPpTMeEY
				[
					172, 29, 45, 130, 196, 166, 155, 22, 195, 206, 158, 181, 208, 182, 243, 79,
					148, 138, 52, 239, 230, 36, 136, 135, 154, 81, 75, 188, 131, 126, 14, 80,
				],
				10000000000,
			),
			(
				// HH5CgPv1RAgQMgpJh6p3NdT9juW5RkTgNTZyY14x3YEEY9W
				[
					208, 5, 206, 5, 204, 134, 218, 235, 225, 186, 153, 99, 120, 74, 102, 208, 84,
					228, 131, 225, 190, 177, 118, 185, 155, 36, 206, 72, 87, 238, 80, 32,
				],
				10000000000000,
			),
			(
				// EibDgpnEGwqvWDcPUq7EThEB6kPEnqKuZog48E5jga8uWe8
				[
					94, 199, 73, 120, 162, 242, 8, 23, 215, 18, 155, 218, 139, 12, 63, 8, 208, 0,
					107, 1, 61, 50, 93, 42, 126, 215, 52, 17, 10, 76, 84, 100,
				],
				10000000000,
			),
			(
				// EUoo6xidm7okewmbh89tUW3sqaDLYKAvTCCNYc8zh4nPU4s
				[
					84, 68, 47, 183, 214, 88, 97, 47, 53, 200, 162, 127, 59, 172, 138, 84, 186, 12,
					104, 77, 138, 180, 223, 185, 173, 134, 130, 69, 143, 101, 109, 60,
				],
				10000000000,
			),
			(
				// HRMhY2CtVMp2yVSieKvq8Y8FHAhRKrGGMd6MoQe3iN6uJ2D
				[
					214, 87, 77, 183, 142, 124, 3, 21, 84, 96, 50, 177, 119, 89, 48, 196, 13, 76,
					202, 117, 98, 233, 33, 12, 189, 233, 51, 68, 170, 215, 170, 84,
				],
				10000000000,
			),
			(
				// DFaiE6wT1caQ9u7eLkuphVQqbywWKzYjm5vRsMgk43GSb84
				[
					29, 241, 183, 189, 149, 102, 45, 153, 129, 74, 88, 239, 30, 85, 203, 179, 88,
					11, 236, 209, 27, 59, 217, 16, 71, 164, 153, 204, 254, 244, 144, 71,
				],
				10000000000,
			),
			(
				// Fgqjkry96qFLpRqPZstNzgaqKXiVyrpzTqD55neMdW8PK6g
				[
					137, 173, 231, 47, 194, 130, 205, 124, 98, 243, 203, 146, 205, 8, 243, 191, 44,
					206, 78, 183, 216, 29, 52, 179, 105, 114, 63, 79, 233, 138, 5, 17,
				],
				10000000000,
			),
			(
				// GUEWJi5DPxDyjmqqGng5PdSCHYALRhtS1v8KT1uCicRbfxk
				[
					172, 76, 151, 41, 241, 24, 246, 253, 252, 221, 211, 235, 39, 43, 238, 2, 40,
					120, 127, 112, 23, 87, 49, 26, 110, 159, 137, 131, 186, 82, 181, 48,
				],
				10000000000,
			),
			(
				// HqFz5RBczgtKrHQGav7DFZwSwrDXxKjLWc95mMDidVdfpwC
				[
					232, 145, 244, 158, 43, 105, 112, 85, 249, 56, 143, 230, 12, 35, 44, 76, 245,
					203, 166, 98, 13, 229, 106, 169, 247, 205, 72, 228, 62, 223, 11, 168,
				],
				10000000000,
			),
			(
				// HnKoshkPTzLwTdKhnTJYL6QgnDaSW4ojiSZdQzj7dzufJ7t
				[
					230, 85, 23, 167, 247, 129, 3, 183, 71, 161, 190, 170, 124, 158, 227, 159, 203,
					254, 25, 148, 140, 142, 7, 128, 223, 0, 12, 31, 242, 196, 114, 28,
				],
				10000000000,
			),
			(
				// G9vLBYmeiQcD8t53djad6sH2MALaeJy9zaEUyknEVma9sa6
				[
					158, 84, 254, 3, 146, 145, 60, 195, 136, 149, 185, 184, 207, 106, 198, 130, 4,
					122, 110, 22, 46, 210, 127, 115, 71, 3, 249, 235, 186, 2, 227, 42,
				],
				10000000000,
			),
			(
				// F9GaWDFcJUUSV7WzrdZ4Zpq8uRFzFQ23hcKrLT9Bdvxzxxe
				[
					113, 153, 171, 173, 152, 127, 178, 8, 186, 128, 74, 4, 122, 115, 23, 37, 195,
					7, 45, 117, 37, 238, 162, 188, 223, 217, 127, 168, 193, 76, 138, 119,
				],
				10000000000,
			),
			(
				// HA5jB52fFL1v4EoEHV4WgEiFZr7wGLiuBDZtSmnwKypvat7
				[
					202, 176, 219, 89, 39, 158, 40, 198, 66, 87, 82, 88, 21, 250, 227, 20, 1, 88,
					104, 69, 191, 189, 43, 142, 116, 157, 109, 105, 63, 204, 250, 13,
				],
				10000000000,
			),
			(
				// EafgFRX24PTgJAjGoaDuQLXQiLX4daSFQQttzGVticSD18o
				[
					88, 188, 84, 163, 42, 93, 248, 215, 47, 249, 108, 212, 218, 212, 67, 74, 181,
					108, 233, 103, 25, 33, 250, 1, 39, 65, 243, 221, 38, 118, 135, 119,
				],
				10000000000,
			),
			(
				// FMTCvCG8qyWbRhb6k8Nu4nJkjYxnUpKY5QzSWfbt1fJH2K6
				[
					122, 228, 100, 83, 103, 128, 129, 0, 139, 109, 242, 81, 134, 220, 106, 165,
					211, 95, 34, 116, 246, 252, 78, 92, 30, 76, 166, 47, 193, 177, 103, 45,
				],
				10000000000,
			),
			(
				// CczSz9z41uHpftVviWz91TgjLe3SmbvXfbAc958cjy7F6Qs
				[
					2, 9, 139, 95, 113, 136, 133, 240, 214, 240, 241, 131, 89, 167, 209, 107, 68,
					201, 34, 152, 87, 147, 78, 254, 102, 218, 244, 217, 240, 235, 122, 67,
				],
				10000000000,
			),
			(
				// EyibGsAttxpNBkgjMxNTArskxkdEFFbwghYuuaZyvu9rmo2
				[
					106, 80, 200, 249, 181, 101, 187, 6, 214, 136, 82, 189, 162, 39, 227, 64, 9,
					157, 73, 90, 63, 230, 16, 209, 252, 187, 179, 226, 179, 131, 134, 9,
				],
				10000000000,
			),
			(
				// Hqa9LGT3qF96agPYYbdfmUzh5P94MX9sqfF1WN4JnfRRVir
				[
					232, 207, 22, 14, 86, 240, 39, 27, 183, 253, 79, 238, 77, 137, 46, 82, 193,
					251, 178, 183, 99, 123, 143, 109, 81, 154, 2, 219, 196, 194, 80, 48,
				],
				10000000000,
			),
			(
				// D2dugfMMD4UEuT1VKwWAEDXYdcdYQ8YyYenqyzov4EQzrWb
				[
					20, 18, 74, 152, 192, 78, 193, 162, 244, 247, 219, 163, 20, 148, 160, 102, 77,
					234, 216, 75, 18, 190, 218, 54, 210, 105, 115, 175, 51, 182, 109, 11,
				],
				10000000000,
			),
			(
				// EzR9J3Afvash2tYCk8ZZwPYyq3zy92adVUXKcYjbYN46JWL
				[
					106, 217, 75, 166, 62, 43, 95, 39, 205, 242, 178, 147, 7, 109, 3, 214, 253,
					255, 44, 20, 164, 97, 54, 104, 211, 243, 117, 150, 167, 140, 152, 71,
				],
				10000000000,
			),
			(
				// H4PFhp41Lf71DshJoTSoHKpVYN8H7VCs3bs9J5oTLAABDci
				[
					198, 88, 98, 42, 56, 161, 58, 36, 180, 89, 254, 109, 16, 255, 214, 120, 192,
					204, 248, 245, 145, 124, 72, 217, 139, 9, 182, 116, 98, 86, 9, 26,
				],
				10000000000,
			),
			(
				// Dtj5dNTPxX6UNfz7DvB3wKUiWcYpjmcDBnjWyYEShgBbtnQ
				[
					58, 69, 248, 95, 254, 189, 177, 143, 25, 199, 92, 139, 237, 97, 234, 17, 219,
					250, 40, 132, 41, 202, 235, 238, 203, 35, 33, 26, 73, 237, 165, 32,
				],
				10000000000,
			),
			(
				// CojQi7xkPUni1Rdde7NmuAxRBxXiFDJA4B824wpvhkeWFP5
				[
					10, 58, 158, 3, 226, 253, 136, 14, 137, 63, 60, 210, 253, 3, 181, 124, 125, 40,
					29, 43, 70, 105, 185, 59, 16, 42, 148, 5, 43, 227, 101, 98,
				],
				10000000000,
			),
			(
				// HRuaGanNmkmeQgZPWPXmkZJb944raNS5ni2vhKzhz75zVYP
				[
					214, 194, 154, 124, 57, 206, 228, 91, 14, 4, 90, 148, 8, 27, 193, 136, 239,
					115, 190, 43, 224, 134, 214, 106, 239, 216, 80, 252, 126, 234, 204, 69,
				],
				10000000000,
			),
			(
				// EakrutF2hSayUqLJYb5wc6qqXkjr8EkGax1UhNzPi1ujBv9
				[
					88, 205, 199, 239, 136, 12, 128, 232, 71, 81, 112, 242, 6, 56, 29, 44, 177, 58,
					135, 194, 9, 69, 47, 198, 216, 161, 225, 65, 134, 214, 27, 40,
				],
				10000000000,
			),
			(
				// HTrpbES27bqMvCioQGHpmJbBzwji6V5DeuXUfB1gsZ5Vkh1
				[
					216, 63, 211, 154, 251, 136, 179, 60, 70, 115, 128, 221, 53, 243, 154, 94, 200,
					71, 74, 119, 110, 210, 175, 19, 52, 249, 165, 252, 167, 14, 0, 250,
				],
				10000000000,
			),
			(
				// Fk3yTFztZdZa4a7yBpisz9ceMyjgYLtZ9CKSCfFNVhoW2ZC
				[
					140, 32, 212, 111, 134, 36, 46, 234, 137, 196, 0, 213, 196, 120, 32, 126, 5,
					199, 107, 186, 178, 154, 116, 138, 248, 170, 201, 13, 98, 126, 26, 1,
				],
				10000000000,
			),
			(
				// GA9YAVVeNdLsToSVa9beeEHtCUncqmT4Pov8h4faQQKR9Fv
				[
					158, 129, 115, 22, 121, 33, 33, 230, 46, 207, 35, 226, 149, 72, 147, 178, 81,
					63, 129, 26, 58, 116, 206, 244, 241, 71, 114, 172, 99, 188, 43, 54,
				],
				10000000000,
			),
			(
				// HqGhgHg6YvnhaXSnaAUvyTDiR4FirB6Ssh2XNDedTzwCDv2
				[
					232, 148, 94, 188, 43, 222, 161, 46, 200, 249, 71, 253, 35, 130, 33, 9, 23, 20,
					110, 38, 238, 99, 169, 248, 31, 132, 26, 83, 213, 104, 77, 66,
				],
				10000000000,
			),
			(
				// GXaUd6gyCaEoBVzXnkLVGneCF3idnLNtNZs5RHTugb9dCpY
				[
					174, 217, 142, 21, 227, 237, 57, 46, 56, 101, 66, 21, 195, 209, 250, 129, 67,
					222, 83, 70, 3, 134, 182, 1, 57, 251, 180, 92, 3, 108, 27, 67,
				],
				10000000000,
			),
			(
				// DMELEF9YvNDscnK6CKQaQ3APYLhTmzhpQkyUf7QhSuNAE3H
				[
					34, 64, 150, 97, 214, 246, 14, 141, 148, 228, 8, 247, 42, 147, 129, 167, 198,
					62, 144, 167, 202, 254, 85, 116, 78, 30, 144, 61, 82, 21, 151, 0,
				],
				10000000000,
			),
			(
				// H1ye1dQ7zVM8obAmb21kfUKA8otRekWXn6fiToKusamaJK9
				[
					196, 130, 101, 105, 230, 139, 126, 238, 27, 91, 147, 64, 110, 73, 81, 252, 215,
					171, 107, 64, 190, 81, 154, 125, 181, 198, 115, 47, 102, 218, 17, 73,
				],
				10000000000000,
			),
			(
				// Gq83oxEvnrHR6Jp6FaoRnTWap1DAGGFC8mtJbA9vCjb4bju
				[
					188, 59, 2, 36, 110, 172, 192, 202, 100, 118, 21, 205, 120, 224, 17, 213, 225,
					34, 73, 18, 118, 25, 22, 51, 60, 72, 29, 235, 42, 156, 233, 89,
				],
				10000000000,
			),
			(
				// ELmaX1aPkyEF7TSmYbbyCjmSgrBpGHv9EtpwR2tk1kmpwvG
				[
					78, 34, 194, 150, 213, 40, 215, 124, 187, 67, 218, 102, 148, 204, 199, 47, 251,
					180, 139, 33, 24, 117, 134, 81, 13, 250, 165, 49, 251, 218, 116, 100,
				],
				10000000000,
			),
			(
				// DimyMqRfrnqudRLVf5TxAM3T7X23PKvFDp85DDGabFKMQ2a
				[
					50, 175, 64, 84, 102, 12, 28, 188, 163, 212, 53, 115, 236, 52, 208, 208, 222,
					200, 46, 196, 174, 31, 235, 68, 6, 123, 72, 142, 37, 227, 15, 47,
				],
				10000000000,
			),
			(
				// DTLcUu92NoQw4gg6VmNgXeYQiNywDhfYMQBPYg2Y1W6AkJF
				[
					38, 233, 51, 113, 227, 226, 183, 195, 139, 229, 42, 201, 30, 142, 166, 33, 165,
					173, 117, 24, 213, 88, 15, 167, 179, 109, 37, 11, 158, 211, 87, 26,
				],
				10000000000,
			),
			(
				// GUuKb4xZuZ13JoDgzWNMC4E1u4166uvkWihvNzZTD3VAHiX
				[
					172, 207, 65, 14, 31, 4, 36, 138, 28, 233, 183, 6, 147, 207, 182, 192, 204,
					118, 71, 241, 30, 225, 18, 128, 106, 53, 79, 187, 2, 204, 186, 10,
				],
				10000000000,
			),
			(
				// FmQHyUXoRkGTRySqVUy7NBAVhkKFvTtRtkzVTjZgBzbDzum
				[
					141, 40, 121, 231, 35, 137, 62, 40, 201, 167, 170, 213, 59, 124, 47, 70, 78,
					135, 1, 155, 54, 216, 77, 153, 11, 228, 80, 154, 46, 118, 198, 73,
				],
				10000000000,
			),
			(
				// Hadx1N8xZq6tRtXWkm6s5madXTXqVhauDNmesZnpMYTufcv
				[
					221, 107, 54, 131, 62, 37, 69, 9, 140, 135, 91, 210, 182, 0, 182, 224, 224,
					233, 42, 167, 21, 60, 26, 16, 39, 67, 41, 99, 185, 219, 192, 7,
				],
				10000000000,
			),
			(
				// CinNnPhc4aGFQb7FWhUpnfwCNwXN3brcnCR1MkazkstcDJa
				[
					6, 116, 96, 152, 14, 88, 143, 217, 64, 53, 184, 138, 34, 250, 75, 50, 44, 231,
					194, 234, 6, 55, 242, 93, 75, 36, 255, 243, 235, 79, 48, 111,
				],
				10000000000,
			),
			(
				// HedLwr1CHmab4QAyoVtxub6kdZDT2YkPDaXawpwfhuCVFjN
				[
					224, 118, 40, 222, 170, 156, 111, 187, 242, 40, 143, 135, 147, 150, 255, 53,
					102, 135, 28, 13, 188, 232, 92, 158, 35, 118, 77, 21, 184, 16, 101, 127,
				],
				10000000000000,
			),
			(
				// GAToWXwmQoMmxHKCmFJ615WbhdGRcRfyDZi7pg7PBRpQuNY
				[
					158, 190, 239, 1, 80, 163, 51, 87, 2, 62, 103, 139, 255, 245, 73, 96, 46, 105,
					67, 181, 184, 93, 139, 253, 181, 132, 115, 153, 47, 207, 175, 99,
				],
				10000000000,
			),
			(
				// JDA8ByXeJcn2BfNabC6WbBJKEBwM4k7oBVenVgJdzL32RJc
				[
					249, 69, 160, 164, 183, 194, 136, 23, 133, 55, 221, 199, 43, 27, 253, 13, 45,
					134, 208, 28, 184, 230, 221, 192, 26, 202, 213, 10, 27, 21, 203, 36,
				],
				10000000000,
			),
			(
				// GC8hwHbQ4TdbYJJPDS96G7Uj9bivnW5z56UEkqujjwhQPp5
				[
					160, 5, 36, 41, 34, 103, 145, 187, 241, 152, 109, 214, 46, 243, 193, 47, 211,
					240, 156, 197, 186, 55, 196, 240, 29, 125, 119, 238, 99, 210, 148, 49,
				],
				10000000000,
			),
			(
				// HutJhhrkoiTJGvHgYj4AYd3vvFYURXENzRTxnezm6brt1xs
				[
					236, 25, 52, 121, 225, 123, 10, 228, 189, 96, 219, 217, 60, 155, 167, 104, 10,
					228, 63, 67, 190, 121, 196, 236, 128, 13, 243, 224, 105, 213, 103, 71,
				],
				10000000000,
			),
			(
				// FcjmeNzPk3vgdENm1rHeiMCxFK96beUoi2kb59FmCoZtkGF
				[
					134, 140, 213, 79, 174, 161, 160, 228, 88, 54, 99, 91, 43, 246, 88, 115, 52,
					54, 236, 105, 197, 86, 125, 101, 27, 229, 146, 57, 44, 187, 105, 220,
				],
				10000000000,
			),
			(
				// FBichC4g5HBmdWCu3ebdADWQbvjbN7KdodNKWBYpLFgxCcd
				[
					115, 119, 207, 242, 138, 62, 20, 248, 72, 21, 155, 51, 44, 84, 161, 114, 156,
					231, 107, 83, 114, 165, 164, 33, 172, 203, 20, 247, 149, 190, 77, 18,
				],
				10000000000,
			),
			(
				// GD7gFyisd4soaCxswDtiB8Db1w6y8MYzyrpkKKAu8M35231
				[
					160, 196, 236, 72, 145, 71, 124, 81, 230, 159, 146, 10, 250, 24, 112, 151, 126,
					187, 51, 129, 250, 94, 174, 2, 141, 125, 250, 22, 204, 112, 0, 119,
				],
				10000000000,
			),
			(
				// Ghw9swKjtCTZfEqEmzZkkqK4vEKQFz86HctEdGprQbNzpc7
				[
					182, 191, 157, 11, 214, 231, 26, 222, 121, 107, 197, 21, 181, 99, 44, 71, 187,
					157, 143, 154, 229, 81, 95, 52, 45, 55, 23, 134, 255, 110, 90, 30,
				],
				10000000000,
			),
			(
				// DmeL34GUWzFPmQ26c5n2RBzZLx2ujZ6A8UqcUqXSynEiXfc
				[
					52, 223, 65, 212, 9, 22, 214, 227, 212, 155, 75, 177, 234, 219, 232, 82, 181,
					4, 154, 49, 106, 254, 246, 250, 31, 246, 168, 79, 175, 159, 82, 62,
				],
				10000000000,
			),
			(
				// ET9SkhNZhY7KT474vkCEJtAjbgJdaqAGW4beeeUJyDQ3SnA
				[
					82, 255, 215, 44, 83, 40, 21, 179, 236, 157, 19, 41, 26, 209, 152, 148, 24, 18,
					156, 62, 137, 73, 154, 145, 142, 136, 77, 91, 243, 251, 176, 53,
				],
				10000000000,
			),
			(
				// DaCSCEQBRmMaBLRQQ5y7swdtfRzjcsewVgCCmngeigwLiax
				[
					44, 36, 100, 44, 239, 20, 231, 115, 21, 191, 70, 124, 0, 145, 124, 116, 154,
					25, 195, 229, 166, 223, 112, 85, 72, 166, 122, 167, 173, 10, 209, 56,
				],
				10000000000,
			),
			(
				// HcPppbUzAiKC2p2eGoQJRsPR576nf9XGjchHzTuMj3nx2kb
				[
					222, 194, 40, 146, 253, 194, 235, 82, 101, 181, 34, 32, 139, 134, 232, 64, 75,
					150, 162, 147, 156, 25, 243, 8, 157, 141, 60, 211, 55, 134, 236, 48,
				],
				10000000000,
			),
			(
				// F4xrhkWsW2PSqZBuBMVJGE3LQy7R2ZWo3J8d6KEsDznrDF8
				[
					110, 81, 9, 40, 228, 132, 254, 3, 110, 108, 191, 214, 140, 165, 11, 2, 167,
					150, 57, 189, 181, 40, 44, 237, 95, 188, 233, 157, 253, 168, 156, 91,
				],
				10000000000,
			),
			(
				// F59ADM2Yxk38AeaVVb2idB3CovryCWc3H1NKXTD8QbJ6pDG
				[
					110, 115, 183, 14, 218, 139, 70, 141, 109, 46, 108, 212, 137, 153, 141, 94,
					222, 178, 203, 156, 34, 89, 117, 7, 133, 138, 221, 78, 97, 157, 149, 120,
				],
				10000000000,
			),
			(
				// DrQHiQu5VkaRuv1H3iELXVqsvD3SV3E8xNjJqXUgECSg23R
				[
					56, 128, 56, 162, 69, 94, 134, 59, 122, 81, 168, 198, 194, 1, 92, 60, 16, 1,
					88, 61, 182, 193, 6, 143, 3, 98, 235, 115, 198, 249, 248, 4,
				],
				10000000000,
			),
			(
				// CpjsLDC1JFyrhm3ftC9Gs4QoyrkHKhZKtK7YqGTRFtTafgp
				[
					10, 255, 104, 101, 99, 90, 225, 16, 19, 168, 56, 53, 192, 25, 212, 78, 195,
					248, 101, 20, 89, 67, 244, 135, 174, 130, 168, 231, 190, 211, 166, 107,
				],
				10000000000,
			),
			(
				// Dm38nyWQsxjS5adN3MzoX1Mu6j38KMZcWSzfMMmmXwsV9Cu
				[
					52, 104, 200, 187, 129, 116, 62, 176, 52, 247, 248, 32, 174, 160, 70, 86, 240,
					60, 168, 60, 101, 92, 144, 175, 197, 203, 123, 46, 164, 82, 42, 6,
				],
				10000000000,
			),
			(
				// FkxWhoWEyJVddzcBwogXQ5VtbdLh8xsAeJwRwjYFem6wMFV
				[
					140, 209, 176, 243, 88, 12, 39, 113, 117, 79, 171, 11, 195, 239, 36, 34, 190,
					180, 127, 75, 245, 5, 35, 10, 57, 100, 235, 83, 223, 37, 192, 27,
				],
				10000000000,
			),
			(
				// Cs1jHXYHxZyWKsGPvYY1BknhLdpNa1iXGja4Kn8hQZs7BsH
				[
					12, 187, 74, 219, 1, 175, 121, 197, 96, 99, 105, 16, 67, 4, 88, 172, 68, 207,
					95, 185, 226, 68, 130, 39, 145, 145, 230, 173, 138, 163, 49, 72,
				],
				10000000000,
			),
			(
				// GZp9fc6uCyoQqu1Bw6DT5riogXskKtCcn9vHW9tZSeB1s3r
				[
					176, 142, 18, 227, 174, 151, 17, 174, 119, 78, 180, 45, 162, 45, 160, 0, 87,
					158, 217, 109, 169, 65, 45, 207, 21, 201, 52, 231, 7, 44, 40, 126,
				],
				10000000000,
			),
			(
				// E8QEnkMMyWWrHPTbA7jo547SpkoGcE6yoTXHmrQeXY2sGnT
				[
					68, 179, 250, 88, 167, 55, 209, 192, 137, 140, 149, 195, 79, 198, 18, 124, 214,
					179, 153, 241, 140, 84, 70, 102, 201, 172, 18, 36, 64, 41, 115, 14,
				],
				10000000000,
			),
			(
				// GPdebankLfiSGaEPQWJBVULEmX2VpNdnyqsa1uiFJGDhTdT
				[
					168, 202, 69, 2, 85, 97, 173, 35, 211, 0, 12, 239, 1, 15, 177, 73, 1, 186, 50,
					34, 131, 137, 142, 8, 195, 208, 167, 252, 221, 3, 206, 90,
				],
				10000000000,
			),
			(
				// HCFcbfVZUpzJ8YwiEno84FG8tH8ms97dSYJZxURx6p8mFQZ
				[
					204, 88, 161, 4, 79, 211, 105, 244, 82, 11, 187, 174, 226, 18, 241, 32, 61,
					124, 179, 97, 27, 84, 80, 153, 243, 137, 134, 27, 145, 28, 2, 90,
				],
				10000000000,
			),
			(
				// Eyvj7oeaHyoqJbNPKPi3zjBks2bNoju3X29xJhsxD8d6GeQ
				[
					106, 121, 162, 203, 24, 32, 88, 130, 95, 169, 119, 223, 202, 52, 55, 120, 129,
					89, 194, 250, 93, 200, 69, 168, 106, 208, 238, 146, 161, 0, 104, 113,
				],
				10000000000,
			),
			(
				// GFE82PQadBP1KhdUTwqfSeRtThUhuYjc8iHGdwMXoAaN3Bn
				[
					162, 97, 27, 101, 196, 115, 166, 134, 30, 13, 237, 211, 142, 107, 20, 138, 87,
					77, 165, 10, 133, 77, 181, 60, 105, 241, 234, 73, 65, 240, 214, 40,
				],
				0,
			),
			(
				// D9rwRxuG8xm8TZf5tgkbPxhhTJK5frCJU9wvp59VRjcMkUf
				[
					25, 148, 223, 91, 240, 244, 67, 66, 177, 113, 155, 251, 177, 86, 18, 134, 189,
					129, 182, 216, 79, 87, 127, 85, 239, 69, 254, 122, 214, 245, 14, 74,
				],
				10000000000,
			),
			(
				// DbuPiksDXhFFEWgjsEghUypTJjQKyULiNESYji3Gaose2NV
				[
					45, 113, 130, 238, 19, 162, 123, 68, 29, 49, 1, 21, 100, 148, 84, 69, 89, 225,
					134, 58, 127, 226, 145, 153, 115, 60, 102, 66, 134, 190, 30, 220,
				],
				10000000000,
			),
			(
				// Fd9kKxogYUZLCoMz3uvjFTCkSGXRvgrKh7GEdbSK2yHd4oq
				[
					134, 221, 140, 72, 101, 126, 156, 176, 232, 62, 68, 214, 48, 200, 225, 207,
					118, 30, 155, 146, 50, 148, 132, 194, 143, 230, 100, 154, 75, 126, 251, 95,
				],
				10000000000,
			),
			(
				// D5Xo7N2jginhYchuMNud2dYtby899koFcaRo2YWNmUquo5H
				[
					22, 71, 114, 192, 73, 219, 243, 18, 244, 59, 179, 240, 129, 255, 55, 239, 226,
					63, 79, 207, 30, 73, 200, 126, 12, 25, 78, 241, 207, 185, 85, 61,
				],
				10000000000000,
			),
			(
				// Fsspzse4QY1KqagdyrVqDt7cmVBr3HSVsfJ38WKgxsLVaXo
				[
					146, 24, 163, 171, 202, 106, 170, 124, 218, 48, 242, 73, 62, 87, 229, 38, 27,
					6, 15, 95, 57, 47, 45, 76, 221, 154, 171, 55, 19, 227, 61, 60,
				],
				10000000000,
			),
			(
				// EUwcW86EFGDoDfUP2UJYuBwhCWC7cW9SdFH9cPh6UPBvBHj
				[
					84, 94, 128, 100, 248, 137, 138, 41, 212, 129, 30, 9, 178, 7, 207, 51, 2, 229,
					206, 254, 241, 102, 21, 248, 88, 15, 205, 143, 166, 58, 98, 78,
				],
				10000000000,
			),
			(
				// DfiSM1qqP11ECaekbA64L2ENcsWEpGk8df8wf1LAfV2sBd4
				[
					48, 89, 157, 186, 80, 181, 243, 186, 11, 54, 248, 86, 167, 97, 235, 60, 10,
					238, 97, 232, 48, 212, 190, 180, 72, 239, 148, 182, 173, 146, 190, 57,
				],
				10000000000,
			),
			(
				// F4cvT6PhxnD3fQyw8x6oXPA2tT7reCd6sTqHt4W5X5pRvHE
				[
					110, 13, 237, 97, 145, 108, 39, 153, 64, 188, 6, 42, 173, 143, 189, 65, 203,
					167, 80, 48, 141, 107, 169, 19, 232, 162, 101, 127, 73, 146, 87, 68,
				],
				10000000000,
			),
			(
				// Etij9aH36W1NjjWbR7wB5j41CmfpqAx8D4V4HCJhUydSH9Y
				[
					102, 129, 3, 218, 239, 82, 46, 105, 22, 6, 78, 232, 226, 125, 179, 58, 185, 80,
					226, 212, 240, 101, 39, 111, 124, 157, 134, 239, 189, 171, 59, 125,
				],
				10000000000,
			),
			(
				// EXAK2eErCzDKK79EA5fDQKtNTRvJLYhphAuBstQ8YrTXpTb
				[
					86, 15, 189, 117, 179, 219, 150, 239, 113, 227, 59, 97, 96, 14, 63, 55, 169,
					38, 64, 8, 135, 218, 170, 174, 56, 13, 54, 54, 148, 156, 7, 103,
				],
				10000000000,
			),
			(
				// HeeJfizAEvorbkinL4GfRUYpxUiFST3dpnUHrh9ga2Z8Cpm
				[
					224, 121, 100, 203, 232, 1, 100, 86, 252, 215, 248, 252, 162, 92, 173, 203, 4,
					92, 94, 114, 186, 225, 244, 221, 152, 80, 73, 57, 214, 213, 165, 53,
				],
				10000000000,
			),
			(
				// Eo9RxTKq2WppUvRRycUmLFHJvHtBVoxURhDe78ZKmjoJfWN
				[
					98, 64, 179, 204, 33, 33, 128, 11, 39, 26, 160, 180, 237, 54, 49, 216, 31, 114,
					140, 17, 80, 141, 23, 117, 49, 94, 210, 145, 31, 236, 86, 105,
				],
				10000000000,
			),
			(
				// FX7rJbfiTFCqBuCWHLgR8SosDyZh842nv79mdQF4vxnZrhS
				[
					130, 67, 176, 152, 110, 162, 112, 65, 79, 118, 140, 232, 93, 84, 179, 99, 199,
					69, 153, 213, 187, 214, 21, 190, 202, 107, 120, 194, 32, 231, 95, 55,
				],
				10000000000,
			),
			(
				// DAT4gSgyMskggCmTQKfEM6hQgRy1NWdhtfMTAuPLsUAGUPy
				[
					26, 7, 191, 89, 235, 128, 113, 81, 98, 128, 119, 89, 38, 44, 176, 61, 151, 78,
					39, 196, 226, 7, 247, 9, 151, 241, 239, 158, 45, 223, 188, 220,
				],
				10000000000,
			),
			(
				// Ce8jPXwfeRfqeKpyzmC2kpoX6vWZw2m8eP22yPCMCEyYxcw
				[
					2, 232, 172, 25, 45, 12, 46, 189, 164, 66, 137, 49, 41, 22, 117, 181, 103, 247,
					231, 17, 150, 44, 7, 224, 150, 240, 212, 155, 146, 21, 197, 107,
				],
				10000000000,
			),
			(
				// FKJNhxaXraoh85DRvChSzoDKHMAk9cZYxoq5LQW7uqeAQMD
				[
					121, 64, 44, 43, 157, 217, 64, 203, 229, 183, 240, 194, 112, 125, 176, 13, 131,
					14, 21, 25, 169, 130, 93, 65, 164, 66, 156, 206, 10, 237, 24, 124,
				],
				10000000000,
			),
			(
				// G7mWyu1Pom5XreLHUzDEcvFp6WaMuLuo4QKxtDB9yJZnH69
				[
					156, 176, 212, 221, 211, 47, 147, 50, 218, 199, 5, 157, 226, 56, 184, 228, 137,
					175, 181, 85, 2, 209, 117, 109, 127, 80, 183, 139, 88, 226, 12, 112,
				],
				10000000000,
			),
			(
				// GhoRyTGK583sJec8aSiyyJCsP2PQXJ2RK7iPGUjLtuX8XCn
				[
					182, 165, 158, 1, 218, 211, 85, 134, 11, 56, 242, 14, 191, 234, 101, 71, 162,
					51, 167, 224, 197, 223, 112, 8, 132, 137, 81, 64, 207, 166, 158, 97,
				],
				10000000000,
			),
			(
				// FfN5P9GqDLuW9oJ57uUZ9bCqNULwkgkLB1jMAEJ4KN7WTwd
				[
					136, 141, 138, 82, 196, 158, 126, 104, 39, 20, 125, 31, 146, 211, 87, 73, 176,
					198, 76, 174, 5, 60, 221, 245, 69, 34, 241, 26, 127, 107, 232, 2,
				],
				10000000000,
			),
			(
				// DMqJZaktTRwAiWRYKPGYEQAyfV7VPrAsBSVDt5CJ1v88Mof
				[
					34, 182, 79, 171, 34, 75, 101, 227, 82, 53, 184, 60, 171, 165, 228, 0, 115,
					111, 243, 6, 246, 159, 183, 132, 189, 133, 70, 70, 124, 58, 254, 67,
				],
				10000000000,
			),
			(
				// Eo4boG437k7gFy75VqPrWP5gHSGi9Sm6CZzLFFsCGYaSPzM
				[
					98, 48, 113, 40, 209, 49, 150, 220, 41, 26, 197, 30, 104, 23, 60, 88, 1, 254,
					107, 172, 227, 3, 162, 34, 210, 101, 146, 85, 217, 222, 191, 39,
				],
				10000000000,
			),
			(
				// HeKaZXya7rPQq7h7KgKnb3jG7n7VBFPr7PFTFYd6ZQbZRiU
				[
					224, 58, 91, 65, 248, 94, 85, 75, 170, 11, 103, 76, 45, 177, 93, 178, 177, 179,
					198, 55, 118, 184, 250, 92, 70, 30, 45, 218, 18, 2, 130, 60,
				],
				10000000000,
			),
			(
				// HMELEC4joprcWnXLMNeVAamceYFZkCXg94Euz86VqsjzF6P
				[
					211, 49, 133, 108, 166, 208, 233, 202, 70, 80, 11, 242, 205, 226, 237, 79, 251,
					134, 3, 157, 151, 156, 127, 31, 216, 11, 209, 25, 27, 43, 164, 85,
				],
				10000000000,
			),
			(
				// EXPniCSoCiE8XhPxsetVm53VD23hrkfPuwHp1VS7Hu6fo9E
				[
					86, 61, 27, 207, 66, 81, 2, 203, 34, 181, 255, 92, 209, 97, 121, 200, 18, 100,
					44, 207, 2, 47, 194, 222, 77, 86, 157, 186, 139, 177, 109, 4,
				],
				10000000000,
			),
			(
				// FRbyyQ55VCnCA3zXPjibseiAYKPhpVo5cDVFaboKw182qYb
				[
					126, 14, 235, 61, 109, 21, 197, 144, 25, 21, 20, 56, 131, 123, 27, 161, 176,
					35, 96, 81, 143, 106, 245, 169, 245, 216, 99, 127, 46, 97, 154, 59,
				],
				10000000000,
			),
			(
				// E457XaKbj2yTB2URy8N4UuzmyuFRkcdxYs67UvSgVr7HyFb
				[
					65, 102, 157, 121, 132, 111, 238, 194, 1, 104, 35, 12, 164, 206, 137, 184, 133,
					22, 44, 247, 214, 252, 170, 229, 54, 147, 220, 243, 241, 183, 222, 244,
				],
				10000000000,
			),
			(
				// D8vZJdm52tKTsfiXcwGXj7nQcXuBQHVGBvPBw6TcH2TfNUB
				[
					24, 221, 206, 222, 170, 178, 160, 16, 221, 180, 199, 113, 236, 20, 25, 192, 55,
					205, 166, 168, 152, 238, 164, 233, 159, 138, 167, 226, 126, 102, 87, 89,
				],
				10000000000000,
			),
			(
				// DNDBcYD8zzqAoZEtgNzouVp2sVxsvqzD4UdB5WrAUwjqpL8
				[
					34, 255, 247, 107, 180, 160, 165, 214, 108, 255, 3, 146, 219, 192, 131, 171,
					186, 195, 179, 4, 111, 111, 204, 50, 138, 191, 13, 221, 22, 202, 8, 55,
				],
				10000000000,
			),
			(
				// EGVQCe73TpFyAZx5uKfE1222XfkT3BSKozjgcqzLBnc5eYo
				[
					74, 223, 81, 164, 123, 114, 121, 83, 102, 213, 34, 133, 227, 41, 34, 156, 131,
					110, 167, 187, 254, 19, 157, 190, 143, 160, 112, 12, 79, 134, 252, 86,
				],
				10000000000,
			),
			(
				// GcqKn3HHodwcFc3Pg3Evcbc43m7qJNMiMv744e5WMSS7TGn
				[
					178, 219, 195, 92, 207, 8, 98, 148, 160, 210, 78, 16, 145, 208, 140, 163, 181,
					194, 164, 135, 7, 28, 79, 181, 64, 112, 230, 102, 204, 153, 224, 45,
				],
				10000000000,
			),
			(
				// DbAdiLJQDFzLyaLsoFCzrpBLuaBXXqQKdpewUSxqiWJadmp
				[
					44, 225, 146, 154, 185, 3, 246, 149, 189, 238, 235, 121, 165, 136, 119, 77,
					113, 70, 131, 98, 18, 145, 54, 241, 183, 247, 179, 26, 50, 149, 143, 152,
				],
				10000000000,
			),
			(
				// DmELeszyrQ19S4SFXPYXRuJheZMFgrfWz3DZ1uZFpGD2LeT
				[
					52, 142, 128, 117, 34, 81, 102, 203, 114, 134, 7, 177, 134, 189, 53, 41, 64,
					30, 58, 159, 240, 251, 75, 183, 108, 125, 117, 141, 24, 41, 82, 41,
				],
				10000000000,
			),
			(
				// DaMuVKJTzz7TNpaFTNSUUGTs5w6mKUHWjJqTWupyqDmCevR
				[
					44, 68, 69, 173, 120, 236, 41, 145, 71, 147, 110, 176, 2, 127, 132, 212, 100,
					135, 196, 218, 172, 126, 104, 88, 138, 144, 106, 174, 50, 61, 249, 99,
				],
				10000000000,
			),
			(
				// HgWWnAXFGikrPVD2FrZ6CRk7KnYdVDn7zVyye8hqFPMc5g1
				[
					225, 229, 168, 53, 106, 31, 6, 6, 126, 94, 246, 40, 97, 249, 219, 25, 46, 101,
					182, 123, 51, 18, 186, 2, 71, 198, 226, 106, 205, 245, 106, 71,
				],
				10000000000,
			),
			(
				// FAn2B22Z3D5cUxvZH9MhXP5VsMhxYd76AHrrLio869y3iLL
				[
					114, 192, 7, 78, 245, 97, 65, 152, 172, 35, 180, 217, 199, 59, 91, 127, 225,
					230, 254, 245, 30, 103, 230, 211, 65, 219, 39, 250, 196, 142, 129, 34,
				],
				10000000000,
			),
			(
				// Eodfj4xjkw8ZFLLSS5RfP6vCMw8aM6qfM7BfeQMf6ivFWHy
				[
					98, 159, 194, 5, 122, 69, 107, 118, 50, 196, 224, 195, 2, 104, 73, 245, 130, 5,
					95, 50, 156, 116, 238, 194, 191, 41, 10, 40, 127, 229, 151, 99,
				],
				10000000000,
			),
			(
				// CsHw8cfzbnKdkCuUq24yuaPyJ1E5a55sNPqejJZ4h7CRtEs
				[
					12, 241, 215, 60, 75, 48, 248, 249, 112, 128, 173, 96, 17, 94, 5, 165, 16, 80,
					37, 32, 29, 186, 208, 233, 94, 36, 92, 109, 120, 244, 56, 5,
				],
				10000000000,
			),
			(
				// F4LocUbsPrcC8xVap4wiTgDakzn3xFyXneuYDHRaHxnb6dH
				[
					109, 215, 171, 105, 161, 188, 205, 45, 187, 120, 36, 135, 248, 1, 80, 155, 243,
					174, 217, 125, 185, 73, 43, 153, 240, 212, 231, 173, 8, 137, 110, 28,
				],
				10000000000,
			),
			(
				// EFeCdfpLENVJJNajoDCvSJ9f9CwutiaDeUFxWjA54kWEG17
				[
					74, 57, 178, 227, 115, 68, 109, 109, 245, 153, 149, 60, 11, 6, 1, 198, 109, 38,
					103, 50, 50, 64, 119, 146, 75, 138, 168, 158, 14, 84, 55, 16,
				],
				10000000000,
			),
			(
				// HqHeKZnc38rX2BJrmJiXfkqHUEUn56B9Nck6WgdiGeKUYBE
				[
					232, 151, 138, 159, 249, 52, 215, 86, 171, 97, 181, 30, 42, 156, 55, 65, 146,
					129, 43, 130, 190, 75, 212, 215, 118, 161, 102, 92, 115, 170, 17, 22,
				],
				10000000000000,
			),
			(
				// CoqysGbay3t3Q7hXgEmGJJquhYYpo8PqLwvW1WsUwR7KvXm
				[
					10, 80, 189, 80, 152, 191, 196, 83, 56, 254, 215, 66, 252, 122, 147, 90, 255,
					158, 208, 88, 197, 55, 123, 32, 17, 101, 133, 144, 127, 16, 98, 1,
				],
				10000000000,
			),
			(
				// HUewJvzVuEeyaxH2vx9XiyAPKrpu1Zj5r5Pi9VrGiBVty7q
				[
					216, 219, 16, 252, 127, 132, 90, 57, 63, 57, 202, 86, 17, 129, 143, 137, 218,
					238, 81, 173, 114, 115, 189, 127, 74, 86, 185, 86, 249, 90, 100, 20,
				],
				10000000000,
			),
			(
				// FSfBJoCU9sRhCYWwQ55iBNGU5L8eu56iGnYGK9zizHxu8dY
				[
					126, 220, 235, 202, 131, 108, 161, 1, 188, 144, 63, 148, 51, 188, 81, 85, 38,
					238, 250, 182, 87, 121, 251, 106, 187, 233, 165, 95, 88, 152, 55, 53,
				],
				10000000000,
			),
			(
				// F8PTaGuZQo5fgRBFuhNnhd5euFiR3KLQNMVhYD5BduPKpHr
				[
					112, 237, 150, 76, 80, 114, 90, 64, 71, 146, 62, 209, 121, 48, 159, 12, 80, 0,
					210, 41, 43, 166, 95, 215, 140, 210, 144, 58, 63, 70, 53, 123,
				],
				10000000000,
			),
			(
				// ELvDjZdLvhbX6FpUbXEWoHbwbxFjxt4BCisp3YRh8UE4Jeu
				[
					78, 63, 217, 189, 49, 72, 151, 246, 158, 18, 14, 56, 41, 71, 93, 226, 204, 190,
					236, 149, 223, 97, 88, 63, 70, 197, 154, 56, 210, 232, 110, 90,
				],
				10000000000,
			),
			(
				// E3YV13RQNELEH1Tbqp2SPkFzirJ8u6rzraTuetDgUDLT4Xd
				[
					64, 255, 129, 147, 44, 86, 190, 113, 168, 32, 124, 138, 153, 50, 141, 96, 165,
					162, 176, 111, 212, 14, 208, 94, 196, 178, 214, 106, 235, 202, 255, 104,
				],
				10000000000,
			),
			(
				// CdEm1ErGKML3waXabLvn3NyqdAGXBQJVngLaM86YM5Yb9dr
				[
					2, 57, 183, 234, 172, 195, 234, 64, 151, 134, 240, 51, 106, 137, 118, 7, 86,
					35, 172, 239, 49, 159, 197, 119, 124, 118, 3, 61, 213, 133, 184, 64,
				],
				10000000000,
			),
			(
				// J6d56aFegPWLLQYoJqxPc4pEexCfxguf4p3FyP2z1ePPQjg
				[
					244, 73, 158, 201, 107, 128, 215, 168, 49, 19, 143, 175, 6, 35, 214, 38, 29,
					232, 230, 81, 175, 150, 7, 5, 24, 160, 59, 53, 111, 188, 53, 1,
				],
				10000000000,
			),
			(
				// ErhkFXudde5xXFVMGUtNpiPLvZ9zcvqM3ueRLukDdpjszys
				[
					100, 247, 56, 209, 242, 26, 97, 78, 233, 145, 186, 173, 45, 125, 175, 216, 128,
					34, 5, 78, 240, 1, 9, 143, 132, 186, 84, 60, 19, 167, 61, 65,
				],
				10000000000,
			),
			(
				// EFjHdypk8xLf3ocDEFPaKFWVcfamH8mpvfUeXHvRWpSBk2M
				[
					74, 74, 210, 31, 116, 72, 227, 24, 60, 24, 184, 49, 79, 254, 207, 11, 56, 32,
					152, 99, 94, 253, 208, 86, 224, 175, 124, 83, 34, 139, 164, 38,
				],
				10000000000,
			),
			(
				// E2ZKmzMzajqW838jXVSM5DyoUJUdEQddXNknEjoTwj2zBLj
				[
					64, 63, 23, 117, 0, 211, 219, 12, 220, 9, 11, 21, 101, 188, 193, 21, 181, 15,
					166, 153, 178, 196, 152, 199, 116, 48, 51, 181, 126, 12, 98, 58,
				],
				10000000000,
			),
			(
				// GcgPeEtLketwNDVVdV2jEnaTU5RMdGQdpYqVshssBWy1txZ
				[
					178, 189, 176, 215, 116, 152, 102, 37, 73, 142, 11, 95, 206, 134, 12, 125, 88,
					16, 59, 219, 107, 123, 52, 128, 84, 213, 37, 253, 220, 63, 62, 127,
				],
				10000000000,
			),
			(
				// J4RcripfeVGe5ZHUfuteYqDdHMkRkLusSfENRKa2kQ2qa96
				[
					242, 156, 147, 199, 55, 226, 66, 210, 201, 235, 35, 226, 52, 10, 26, 206, 87,
					153, 192, 198, 121, 146, 241, 90, 44, 186, 213, 45, 242, 220, 123, 113,
				],
				10000000000,
			),
			(
				// D8BfryaM5xN62UuKUpLK5zbZEUSBtA76yP9YddQTKXi9pkB
				[
					24, 77, 112, 18, 149, 190, 123, 179, 139, 44, 12, 88, 163, 91, 248, 237, 197,
					146, 103, 28, 83, 209, 73, 210, 6, 224, 55, 220, 124, 155, 235, 123,
				],
				10000000000,
			),
			(
				// GFrQQ9cMz6mHdZig9bBJdNmP4CySJvefywouoBa6hdCUyjQ
				[
					162, 219, 62, 134, 60, 242, 117, 166, 56, 13, 214, 181, 253, 61, 15, 90, 130,
					152, 127, 31, 70, 121, 10, 175, 193, 162, 219, 145, 58, 181, 57, 71,
				],
				10000000000,
			),
			(
				// DwZBcfHnJtRmR7P23VgHQEzaeGPXQvH8jDvuob2qiyTHJMM
				[
					60, 110, 100, 21, 157, 35, 76, 116, 134, 198, 129, 186, 157, 48, 44, 60, 236,
					65, 80, 253, 201, 189, 178, 104, 142, 151, 68, 15, 207, 241, 7, 40,
				],
				10000000000,
			),
			(
				// CrzGYAYYnguxoR5pGx4UbwLs2DkoxoLiLJd8kjZMQzDuq8r
				[
					12, 182, 95, 169, 151, 22, 156, 224, 71, 110, 30, 124, 18, 113, 125, 75, 46,
					116, 241, 87, 125, 218, 224, 2, 223, 182, 118, 63, 87, 242, 216, 3,
				],
				10000000000,
			),
			(
				// FagAVsTYT8QghxypUtLcfnmnnhPhPpf854UNuptpQKuNndK
				[
					132, 250, 52, 33, 111, 141, 173, 223, 29, 75, 206, 46, 56, 72, 71, 124, 222,
					140, 211, 139, 226, 179, 165, 161, 38, 254, 36, 111, 96, 86, 124, 60,
				],
				10000000000000,
			),
			(
				// DMELEokhPzoNDxYNpNPCfEoLs3BEYX297aE733Nzo3UAtVq
				[
					34, 64, 150, 134, 181, 22, 178, 185, 118, 57, 208, 49, 144, 158, 138, 150, 173,
					134, 233, 98, 130, 148, 157, 2, 15, 104, 100, 29, 63, 193, 250, 121,
				],
				10000000000,
			),
			(
				// EqyCQvYn1cHBdzFVQHQeL1nHDcxHhjWR8V48KbDyHyuyCGV
				[
					100, 103, 253, 78, 112, 56, 185, 37, 194, 66, 35, 87, 56, 13, 140, 192, 197,
					241, 125, 39, 47, 99, 154, 248, 252, 253, 31, 17, 86, 222, 112, 64,
				],
				10000000000,
			),
		];
		let count = accounts_and_deposits.len() as u64;
		accounts_and_deposits.into_iter().map(|(who, deposit)| (AccountId::from(who), deposit)).for_each(|(who, deposit)| {
			use frame_support::traits::ReservableCurrency;
			use sp_runtime::traits::Zero;
			let leftover = Balances::unreserve(&who, deposit);
			if check && !leftover.is_zero() {
				log::warn!(
					target: "runtime::kusama",
					"some account {:?} does not have enough balance for the refund ({}), this is not necessarily a problem.",
					who,
					leftover,
				);
			}
		});

		<Runtime as frame_system::Config>::DbWeight::get().reads_writes(count, count)
	}
}

impl OnRuntimeUpgrade for RefundNickPalletDeposit {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		if VERSION.spec_version == 9150 {
			log::info!(target: "runtime::kusama", "executing the refund migration of https://github.com/paritytech/polkadot/pull/4656");
			Self::execute(false)
		} else {
			log::warn!(target: "runtime::kusama", "RefundNickPalletDeposit should be removed");
			0
		}
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<(), &'static str> {
		let _ = Self::execute(true);
		Ok(())
	}
}

// Migration for scheduler pallet to move from a plain Call to a CallOrHash.
pub struct SchedulerMigrationV3;

impl OnRuntimeUpgrade for SchedulerMigrationV3 {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		Scheduler::migrate_v2_to_v3()
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<(), &'static str> {
		Scheduler::pre_migrate_to_v3()
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade() -> Result<(), &'static str> {
		Scheduler::post_migrate_to_v3()
	}
}

/// Migrate session-historical from `Session` to the new pallet prefix `Historical`
pub struct SessionHistoricalPalletPrefixMigration;

impl OnRuntimeUpgrade for SessionHistoricalPalletPrefixMigration {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		pallet_session::migrations::v1::migrate::<Runtime, Historical>()
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<(), &'static str> {
		pallet_session::migrations::v1::pre_migrate::<Runtime, Historical>();
		Ok(())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade() -> Result<(), &'static str> {
		pallet_session::migrations::v1::post_migrate::<Runtime, Historical>();
		Ok(())
	}
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

	impl primitives::v2::ParachainHost<Block, Hash, BlockNumber> for Runtime {
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
			outputs: primitives::v1::CandidateCommitments,
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
					Event::ParaInclusion(ev) => {
						Some(ev)
					}
					_ => None,
				}
			})
		}

		fn session_info(index: SessionIndex) -> Option<SessionInfo> {
			parachains_runtime_api_impl::session_info::<Runtime>(index)
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
			stmt: primitives::v2::PvfCheckStatement,
			signature: primitives::v1::ValidatorSignature,
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
	}

	impl beefy_primitives::BeefyApi<Block> for Runtime {
		fn validator_set() -> Option<beefy_primitives::ValidatorSet<BeefyId>> {
			// dummy implementation due to lack of BEEFY pallet.
			None
		}
	}

	impl mmr::MmrApi<Block, Hash> for Runtime {
		fn generate_proof(_leaf_index: u64)
			-> Result<(mmr::EncodableOpaqueLeaf, mmr::Proof<Hash>), mmr::Error>
		{
			// dummy implementation due to lack of MMR pallet.
			Err(mmr::Error::GenerateProof)
		}

		fn verify_proof(_leaf: mmr::EncodableOpaqueLeaf, _proof: mmr::Proof<Hash>)
			-> Result<(), mmr::Error>
		{
			// dummy implementation due to lack of MMR pallet.
			Err(mmr::Error::Verify)
		}

		fn verify_proof_stateless(
			_root: Hash,
			_leaf: mmr::EncodableOpaqueLeaf,
			_proof: mmr::Proof<Hash>
		) -> Result<(), mmr::Error> {
			// dummy implementation due to lack of MMR pallet.
			Err(mmr::Error::Verify)
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
		fn configuration() -> babe_primitives::BabeGenesisConfiguration {
			// The choice of `c` parameter (where `1 - c` represents the
			// probability of a slot being empty), is done in accordance to the
			// slot duration and expected target block time, for safely
			// resisting network delays of maximum two seconds.
			// <https://research.web3.foundation/en/latest/polkadot/BABE/Babe/#6-practical-results>
			babe_primitives::BabeGenesisConfiguration {
				slot_duration: Babe::slot_duration(),
				epoch_length: EpochDuration::get(),
				c: BABE_GENESIS_EPOCH_CONFIG.c,
				genesis_authorities: Babe::authorities().to_vec(),
				randomness: Babe::randomness(),
				allowed_slots: BABE_GENESIS_EPOCH_CONFIG.allowed_slots,
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
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade() -> (Weight, Weight) {
			log::info!("try-runtime::on_runtime_upgrade kusama.");
			let weight = Executive::try_runtime_upgrade().unwrap();
			(weight, BlockWeights::get().max_block)
		}
		fn execute_block_no_check(block: Block) -> Weight {
			Executive::execute_block_no_check(block)
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{list_benchmark, Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;

			use pallet_session_benchmarking::Pallet as SessionBench;
			use pallet_offences_benchmarking::Pallet as OffencesBench;
			use frame_system_benchmarking::Pallet as SystemBench;

			let mut list = Vec::<BenchmarkList>::new();

			// Polkadot
			// NOTE: Make sure to prefix these `runtime_common::` so that path resolves correctly
			// in the generated file.
			list_benchmark!(list, extra, runtime_common::auctions, Auctions);
			list_benchmark!(list, extra, runtime_common::crowdloan, Crowdloan);
			list_benchmark!(list, extra, runtime_common::claims, Claims);
			list_benchmark!(list, extra, runtime_common::slots, Slots);
			list_benchmark!(list, extra, runtime_common::paras_registrar, Registrar);
			list_benchmark!(list, extra, runtime_parachains::configuration, Configuration);
			list_benchmark!(list, extra, runtime_parachains::hrmp, Hrmp);
			list_benchmark!(list, extra, runtime_parachains::disputes, ParasDisputes);
			list_benchmark!(list, extra, runtime_parachains::initializer, Initializer);
			list_benchmark!(list, extra, runtime_parachains::paras_inherent, ParaInherent);
			list_benchmark!(list, extra, runtime_parachains::paras, Paras);
			// Substrate
			list_benchmark!(list, extra, pallet_bags_list, BagsList);
			list_benchmark!(list, extra, pallet_balances, Balances);
			list_benchmark!(list, extra, pallet_bounties, Bounties);
			list_benchmark!(list, extra, pallet_collective, Council);
			list_benchmark!(list, extra, pallet_collective, TechnicalCommittee);
			list_benchmark!(list, extra, pallet_democracy, Democracy);
			list_benchmark!(list, extra, pallet_elections_phragmen, PhragmenElection);
			list_benchmark!(list, extra, pallet_election_provider_multi_phase, ElectionProviderMultiPhase);
			list_benchmark!(list, extra, pallet_gilt, Gilt);
			list_benchmark!(list, extra, pallet_identity, Identity);
			list_benchmark!(list, extra, pallet_im_online, ImOnline);
			list_benchmark!(list, extra, pallet_indices, Indices);
			list_benchmark!(list, extra, pallet_membership, TechnicalMembership);
			list_benchmark!(list, extra, pallet_multisig, Multisig);
			list_benchmark!(list, extra, pallet_offences, OffencesBench::<Runtime>);
			list_benchmark!(list, extra, pallet_preimage, Preimage);
			list_benchmark!(list, extra, pallet_proxy, Proxy);
			list_benchmark!(list, extra, pallet_scheduler, Scheduler);
			list_benchmark!(list, extra, pallet_session, SessionBench::<Runtime>);
			list_benchmark!(list, extra, pallet_staking, Staking);
			list_benchmark!(list, extra, frame_system, SystemBench::<Runtime>);
			list_benchmark!(list, extra, pallet_timestamp, Timestamp);
			list_benchmark!(list, extra, pallet_tips, Tips);
			list_benchmark!(list, extra, pallet_treasury, Treasury);
			list_benchmark!(list, extra, pallet_utility, Utility);
			list_benchmark!(list, extra, pallet_vesting, Vesting);

			let storage_info = AllPalletsWithSystem::storage_info();

			return (list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<
			Vec<frame_benchmarking::BenchmarkBatch>,
			sp_runtime::RuntimeString,
		> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, add_benchmark, TrackedStorageKey};
			// Trying to add benchmarks directly to some pallets caused cyclic dependency issues.
			// To get around that, we separated the benchmarks into its own crate.
			use pallet_session_benchmarking::Pallet as SessionBench;
			use pallet_offences_benchmarking::Pallet as OffencesBench;
			use frame_system_benchmarking::Pallet as SystemBench;

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
			// NOTE: Make sure to prefix these `runtime_common::` so that path resolves correctly
			// in the generated file.
			add_benchmark!(params, batches, runtime_common::auctions, Auctions);
			add_benchmark!(params, batches, runtime_common::crowdloan, Crowdloan);
			add_benchmark!(params, batches, runtime_common::claims, Claims);
			add_benchmark!(params, batches, runtime_common::slots, Slots);
			add_benchmark!(params, batches, runtime_common::paras_registrar, Registrar);
			add_benchmark!(params, batches, runtime_parachains::configuration, Configuration);
			add_benchmark!(params, batches, runtime_parachains::hrmp, Hrmp);
			add_benchmark!(params, batches, runtime_parachains::disputes, ParasDisputes);
			add_benchmark!(params, batches, runtime_parachains::initializer, Initializer);
			add_benchmark!(params, batches, runtime_parachains::paras_inherent, ParaInherent);
			add_benchmark!(params, batches, runtime_parachains::paras, Paras);
			// Substrate
			add_benchmark!(params, batches, pallet_balances, Balances);
			add_benchmark!(params, batches, pallet_bags_list, BagsList);
			add_benchmark!(params, batches, pallet_bounties, Bounties);
			add_benchmark!(params, batches, pallet_collective, Council);
			add_benchmark!(params, batches, pallet_collective, TechnicalCommittee);
			add_benchmark!(params, batches, pallet_democracy, Democracy);
			add_benchmark!(params, batches, pallet_elections_phragmen, PhragmenElection);
			add_benchmark!(params, batches, pallet_election_provider_multi_phase, ElectionProviderMultiPhase);
			add_benchmark!(params, batches, pallet_gilt, Gilt);
			add_benchmark!(params, batches, pallet_identity, Identity);
			add_benchmark!(params, batches, pallet_im_online, ImOnline);
			add_benchmark!(params, batches, pallet_indices, Indices);
			add_benchmark!(params, batches, pallet_membership, TechnicalMembership);
			add_benchmark!(params, batches, pallet_multisig, Multisig);
			add_benchmark!(params, batches, pallet_offences, OffencesBench::<Runtime>);
			add_benchmark!(params, batches, pallet_preimage, Preimage);
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
mod tests_fess {
	use super::*;
	use sp_runtime::assert_eq_error_rate;

	#[test]
	fn signed_deposit_is_sensible() {
		// ensure this number does not change, or that it is checked after each change.
		// a 1 MB solution should need around 0.16 KSM deposit
		let deposit = SignedDepositBase::get() + (SignedDepositByte::get() * 1024 * 1024);
		assert_eq_error_rate!(deposit, UNITS * 16 / 100, UNITS / 100);
	}
}
