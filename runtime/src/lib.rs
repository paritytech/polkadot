// Copyright 2017 Parity Technologies (UK) Ltd.
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

mod attestations;
mod claims;
mod parachains;
mod slot_range;
mod registrar;
mod slots;
mod crowdfund;

use rstd::prelude::*;
use substrate_primitives::u32_trait::{_1, _2, _3, _4, _5};
use codec::{Encode, Decode};
use primitives::{
	AccountId, AccountIndex, Balance, BlockNumber, Hash, Nonce, Signature, Moment,
	parachain::{self, ActiveParas}, ValidityError,
};
use client::{
	block_builder::api::{self as block_builder_api, InherentData, CheckInherentsResult},
	runtime_api as client_api, impl_runtime_apis,
};
use sr_primitives::{
	create_runtime_str, generic, impl_opaque_keys,  key_types,
	ApplyResult, Permill, Perbill, RuntimeDebug,
	transaction_validity::{TransactionValidity, InvalidTransaction, TransactionValidityError},
	weights::{Weight, DispatchInfo}, curve::PiecewiseLinear,
	traits::{BlakeTwo256, Block as BlockT, StaticLookup, SignedExtension},
};
use version::RuntimeVersion;
use grandpa::{AuthorityId as GrandpaId, fg_primitives};
use babe_primitives::{AuthorityId as BabeId};
#[cfg(any(feature = "std", test))]
use version::NativeVersion;
use substrate_primitives::OpaqueMetadata;
use sr_staking_primitives::SessionIndex;
use srml_support::{
	parameter_types, construct_runtime, traits::{SplitTwoWays, Currency, Randomness}
};
use im_online::sr25519::AuthorityId as ImOnlineId;
use system::offchain::TransactionSubmitter;

#[cfg(feature = "std")]
pub use staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use sr_primitives::BuildStorage;
pub use timestamp::Call as TimestampCall;
pub use balances::Call as BalancesCall;
pub use attestations::{Call as AttestationsCall, MORE_ATTESTATIONS_IDENTIFIER};
pub use parachains::{Call as ParachainsCall, NEW_HEADS_IDENTIFIER};

/// Implementations of some helper traits passed into runtime modules as associated types.
pub mod impls;
use impls::{CurrencyToVoteHandler, TargetedFeeAdjustment, ToAuthor, WeightToFee};

/// Constant values used within the runtime.
pub mod constants;
use constants::{time::*, currency::*};

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/*
// KUSAMA: Polkadot version identifier; may be uncommented for Polkadot mainnet.
/// Runtime version (Polkadot).
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("polkadot"),
	impl_name: create_runtime_str!("parity-polkadot"),
	authoring_version: 1,
	spec_version: 1000,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
};
*/

// KUSAMA: Kusama version identifier; may be removed for Polkadot mainnet.
/// Runtime version (Kusama).
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("kusama"),
	impl_name: create_runtime_str!("parity-kusama"),
	authoring_version: 1,
	spec_version: 1006,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
};

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion {
		runtime_version: VERSION,
		can_author_with: Default::default(),
	}
}

/// Avoid processing transactions that are anything except staking and claims.
///
/// RELEASE: This is only relevant for the initial PoA run-in period and may be removed
/// from the release runtime.
#[derive(Default, Encode, Decode, Clone, Eq, PartialEq, RuntimeDebug)]
pub struct OnlyStakingAndClaims;
impl SignedExtension for OnlyStakingAndClaims {
	type AccountId = AccountId;
	type Call = Call;
	type AdditionalSigned = ();
	type Pre = ();
	fn additional_signed(&self) -> rstd::result::Result<(), TransactionValidityError> { Ok(()) }
	fn validate(&self, _: &Self::AccountId, call: &Self::Call, _: DispatchInfo, _: usize)
		-> TransactionValidity
	{
		match call {
			Call::Staking(_) | Call::Claims(_) | Call::Sudo(_) | Call::Session(_)
				| Call::ElectionsPhragmen(_) | Call::TechnicalMembership(_)
				| Call::TechnicalCommittee(_) | Call::Nicks(_)
			=>
				Ok(Default::default()),
			_ => Err(InvalidTransaction::Custom(ValidityError::NoPermission.into()).into()),
		}
	}
}

type NegativeImbalance = <Balances as Currency<AccountId>>::NegativeImbalance;

parameter_types! {
	pub const BlockHashCount: BlockNumber = 250;
	pub const MaximumBlockWeight: Weight = 1_000_000_000;
	pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	pub const MaximumBlockLength: u32 = 5 * 1024 * 1024;
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
	type Lookup = Indices;
	type Header = generic::Header<BlockNumber, BlakeTwo256>;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type MaximumBlockWeight = MaximumBlockWeight;
	type MaximumBlockLength = MaximumBlockLength;
	type AvailableBlockRatio = AvailableBlockRatio;
	type Version = Version;
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

impl indices::Trait for Runtime {
	type IsDeadAccount = Balances;
	type AccountIndex = AccountIndex;
	type ResolveHint = indices::SimpleResolveHint<Self::AccountId, Self::AccountIndex>;
	type Event = Event;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 100 * CENTS;
	pub const TransferFee: Balance = 1 * CENTS;
	pub const CreationFee: Balance = 1 * CENTS;
}

/// Splits fees 80/20 between treasury and block author.
pub type DealWithFees = SplitTwoWays<
	Balance,
	NegativeImbalance,
	_4, Treasury,   // 4 parts (80%) goes to the treasury.
	_1, ToAuthor,   // 1 part (20%) goes to the block author.
>;

impl balances::Trait for Runtime {
	type Balance = Balance;
	type OnFreeBalanceZero = Staking;
	type OnNewAccount = Indices;
	type Event = Event;
	type DustRemoval = ();
	type TransferPayment = ();
	type ExistentialDeposit = ExistentialDeposit;
	type TransferFee = TransferFee;
	type CreationFee = CreationFee;
}

parameter_types! {
	pub const TransactionBaseFee: Balance = 1 * CENTS;
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
	// for a sane configuration, this should always be less than `AvailableBlockRatio`.
	pub const TargetBlockFullness: Perbill = Perbill::from_percent(25);
}

impl transaction_payment::Trait for Runtime {
	type Currency = Balances;
	type OnTransactionPayment = DealWithFees;
	type TransactionBaseFee = TransactionBaseFee;
	type TransactionByteFee = TransactionByteFee;
	type WeightToFee = WeightToFee;
	type FeeMultiplierUpdate = TargetedFeeAdjustment<TargetBlockFullness>;
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
	type EventHandler = Staking;
}

parameter_types! {
	pub const Period: BlockNumber = 10 * MINUTES;
	pub const Offset: BlockNumber = 0;
}

// !!!!!!!!!!!!!
// WARNING!!!!!!  SEE NOTE BELOW BEFORE TOUCHING THIS CODE
// !!!!!!!!!!!!!
type SessionHandlers = (Grandpa, Babe, ImOnline, Parachains);
impl_opaque_keys! {
	pub struct SessionKeys {
		#[id(key_types::GRANDPA)]
		pub grandpa: GrandpaId,
		#[id(key_types::BABE)]
		pub babe: BabeId,
		#[id(key_types::IM_ONLINE)]
		pub im_online: ImOnlineId,
		#[id(parachain::PARACHAIN_KEY_TYPE_ID)]
		pub parachain_validator: parachain::ValidatorId,
	}
}
// NOTE: `SessionHandler` and `SessionKeys` are co-dependent: One key will be used for each handler.
// The number and order of items in `SessionHandler` *MUST* be the same number and order of keys in
// `SessionKeys`.
// TODO: Introduce some structure to tie these together to make it a bit less of a footgun. This
// should be easy, since OneSessionHandler trait provides the `Key` as an associated type. #2858

parameter_types! {
	pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
}

impl session::Trait for Runtime {
	type OnSessionEnding = Staking;
	type SessionHandler = SessionHandlers;
	type ShouldEndSession = Babe;
	type Event = Event;
	type Keys = SessionKeys;
	type ValidatorId = AccountId;
	type ValidatorIdOf = staking::StashOf<Self>;
	type SelectInitialValidators = Staking;
	type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
}

impl session::historical::Trait for Runtime {
	type FullIdentification = staking::Exposure<AccountId, Balance>;
	type FullIdentificationOf = staking::ExposureOf<Runtime>;
}

srml_staking_reward_curve::build! {
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
	// KUSAMA: This value is 1/4 of what we expect for the mainnet.
	pub const BondingDuration: staking::EraIndex = 7;
	pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
}

impl staking::Trait for Runtime {
	type OnRewardMinted = Treasury;
	type CurrencyToVote = CurrencyToVoteHandler;
	type Event = Event;
	type Currency = Balances;
	type Slash = Treasury;
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SessionInterface = Self;
	type Time = Timestamp;
	type RewardCurve = RewardCurve;
}

parameter_types! {
	// KUSAMA: These values are 1/4 of what we expect for the mainnet.
	pub const LaunchPeriod: BlockNumber = 7 * DAYS;
	pub const VotingPeriod: BlockNumber = 7 * DAYS;
	pub const EmergencyVotingPeriod: BlockNumber = 3 * HOURS;
	pub const MinimumDeposit: Balance = 100 * DOLLARS;
	pub const EnactmentPeriod: BlockNumber = 8 * DAYS;
	pub const CooloffPeriod: BlockNumber = 7 * DAYS;
}

impl democracy::Trait for Runtime {
	type Proposal = Call;
	type Event = Event;
	type Currency = Balances;
	type EnactmentPeriod = EnactmentPeriod;
	type LaunchPeriod = LaunchPeriod;
	type VotingPeriod = VotingPeriod;
	type EmergencyVotingPeriod = EmergencyVotingPeriod;
	type MinimumDeposit = MinimumDeposit;
	/// A straight majority of the council can decide what their next motion is.
	type ExternalOrigin = collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilCollective>;
	/// A super-majority can have the next scheduled referendum be a straight majority-carries vote.
	type ExternalMajorityOrigin = collective::EnsureProportionAtLeast<_3, _4, AccountId, CouncilCollective>;
	/// A unanimous council can have the next scheduled referendum be a straight default-carries
	/// (NTB) vote.
	type ExternalDefaultOrigin = collective::EnsureProportionAtLeast<_1, _1, AccountId, CouncilCollective>;
	/// Two thirds of the technical committee can have an ExternalMajority/ExternalDefault vote
	/// be tabled immediately and with a shorter voting/enactment period.
	type FastTrackOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, TechnicalCollective>;
	// To cancel a proposal which has been passed, 2/3 of the council must agree to it.
	type CancellationOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilCollective>;
	// Any single technical committee member may veto a coming council proposal, however they can
	// only do it once and it lasts only for the cooloff period.
	type VetoOrigin = collective::EnsureMember<AccountId, TechnicalCollective>;
	type CooloffPeriod = CooloffPeriod;
}

type CouncilCollective = collective::Instance1;
impl collective::Trait<CouncilCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
}

parameter_types! {
	pub const CandidacyBond: Balance = 100 * DOLLARS;
	pub const VotingBond: Balance = 5 * DOLLARS;
}

impl elections_phragmen::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type ChangeMembers = Council;
	type CurrencyToVote = CurrencyToVoteHandler;
	type CandidacyBond = CandidacyBond;
	type VotingBond = VotingBond;
	type LoserCandidate = Treasury;
	type BadReport = Treasury;
	type KickedMember = Treasury;
}

type TechnicalCollective = collective::Instance2;
impl collective::Trait<TechnicalCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
}

impl membership::Trait<membership::Instance1> for Runtime {
	type Event = Event;
	type AddOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type RemoveOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type SwapOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type ResetOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type MembershipInitialized = TechnicalCommittee;
	type MembershipChanged = TechnicalCommittee;
}

parameter_types! {
	pub const ProposalBond: Permill = Permill::from_percent(5);
	pub const ProposalBondMinimum: Balance = 100 * DOLLARS;
	// KUSAMA: This value is 1/4 of that expected for mainnet
	pub const SpendPeriod: BlockNumber = 6 * DAYS;
	// KUSAMA: This value is 1/5 of that expected for mainnet
	pub const Burn: Permill = Permill::from_percent(1);
}

impl treasury::Trait for Runtime {
	type Currency = Balances;
	type ApproveOrigin = collective::EnsureProportionAtLeast<_3, _5, AccountId, CouncilCollective>;
	type RejectOrigin = collective::EnsureProportionMoreThan<_1, _2, AccountId, CouncilCollective>;
	type Event = Event;
	type ProposalRejection = Treasury;
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type SpendPeriod = SpendPeriod;
	type Burn = Burn;
}

impl offences::Trait for Runtime {
	type Event = Event;
	type IdentificationTuple = session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = Staking;
}

type SubmitTransaction = TransactionSubmitter<ImOnlineId, Runtime, UncheckedExtrinsic>;

impl im_online::Trait for Runtime {
	type AuthorityId = ImOnlineId;
	type Event = Event;
	type Call = Call;
	type SubmitTransaction = SubmitTransaction;
	type ReportUnresponsiveness = ();
}

impl grandpa::Trait for Runtime {
	type Event = Event;
}

parameter_types! {
	pub const WindowSize: BlockNumber = finality_tracker::DEFAULT_WINDOW_SIZE.into();
	pub const ReportLatency: BlockNumber = finality_tracker::DEFAULT_REPORT_LATENCY.into();
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

impl parachains::Trait for Runtime {
	type Origin = Origin;
	type Call = Call;
	type ParachainCurrency = Balances;
	type Randomness = RandomnessCollectiveFlip;
	type ActiveParachains = Registrar;
	type Registrar = Registrar;
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

parameter_types!{
	pub const LeasePeriod: BlockNumber = 100_000;
	pub const EndingPeriod: BlockNumber = 1000;
}

impl slots::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Parachains = Registrar;
	type LeasePeriod = LeasePeriod;
	type EndingPeriod = EndingPeriod;
	type Randomness = RandomnessCollectiveFlip;
}

parameter_types!{
	// KUSAMA: for mainnet this should be removed.
	pub const Prefix: &'static [u8] = b"Pay KSMs to the Kusama account:";
	// KUSAMA: for mainnet this should be uncommented.
	//pub const Prefix: &'static [u8] = b"Pay DOTs to the Polkadot account:";
}

impl claims::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Prefix = Prefix;
}

impl sudo::Trait for Runtime {
	type Event = Event;
	type Proposal = Call;
}

parameter_types! {
	pub const ReservationFee: Balance = 1 * DOLLARS;
	pub const MinLength: usize = 3;
	pub const MaxLength: usize = 16;
}

impl nicks::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type ReservationFee = ReservationFee;
	type Slashed = Treasury;
	type KillOrigin = collective::EnsureMember<AccountId, CouncilCollective>;
	type MinLength = MinLength;
	type MaxLength = MaxLength;
}

construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = primitives::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		// Basic stuff; balances is uncallable initially.
		System: system::{Module, Call, Storage, Config, Event},
		RandomnessCollectiveFlip: randomness_collective_flip::{Module, Storage},

		// Must be before session.
		Babe: babe::{Module, Call, Storage, Config, Inherent(Timestamp)},

		Timestamp: timestamp::{Module, Call, Storage, Inherent},
		Indices: indices,
		Balances: balances::{Module, Call, Storage, Config<T>, Event<T>},
		TransactionPayment: transaction_payment::{Module, Storage},

		// Consensus support.
		Authorship: authorship::{Module, Call, Storage},
		Staking: staking::{default, OfflineWorker},
		Offences: offences::{Module, Call, Storage, Event},
		Session: session::{Module, Call, Storage, Event, Config<T>},
		FinalityTracker: finality_tracker::{Module, Call, Inherent},
		Grandpa: grandpa::{Module, Call, Storage, Config, Event},
		ImOnline: im_online::{Module, Call, Storage, Event<T>, ValidateUnsigned, Config<T>},

		// Governance stuff; uncallable initially.
		Democracy: democracy::{Module, Call, Storage, Config, Event<T>},
		Council: collective::<Instance1>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>},
		TechnicalCommittee: collective::<Instance2>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>},
		ElectionsPhragmen: elections_phragmen::{Module, Call, Storage, Event<T>, Config<T>},
		TechnicalMembership: membership::<Instance1>::{Module, Call, Storage, Event<T>, Config<T>},
		Treasury: treasury::{Module, Call, Storage, Event<T>},

		// Claims. Usable initially.
		Claims: claims::{Module, Call, Storage, Event<T>, Config<T>, ValidateUnsigned},

		// Parachains stuff; slots are disabled (no auctions initially). The rest are safe as they
		// have no public dispatchables.
		Parachains: parachains::{Module, Call, Storage, Config, Inherent, Origin},
		Attestations: attestations::{Module, Call, Storage},
		Slots: slots::{Module, Call, Storage, Event<T>},
		Registrar: registrar::{Module, Call, Storage, Event, Config<T>},

		// Sudo. Usable initially.
		// RELEASE: remove this for release build.
		Sudo: sudo,

		// Simple nicknames module.
		Nicks: nicks::{Module, Call, Storage, Event<T>},
	}
);

/// The address format for describing accounts.
pub type Address = <Indices as StaticLookup>::Source;
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
	// RELEASE: remove this for release build.
	OnlyStakingAndClaims,
	system::CheckVersion<Runtime>,
	system::CheckGenesis<Runtime>,
	system::CheckEra<Runtime>,
	system::CheckNonce<Runtime>,
	system::CheckWeight<Runtime>,
	transaction_payment::ChargeTransactionPayment::<Runtime>,
	registrar::LimitParathreadCommits<Runtime>
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<Address, Call, Signature, SignedExtra>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Nonce, Call>;
/// Executive: handles dispatch to the various modules.
pub type Executive = executive::Executive<Runtime, Block, system::ChainContext<Runtime>, Runtime, AllModules>;

impl_runtime_apis! {
	impl client_api::Core<Block> for Runtime {
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

	impl client_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			Runtime::metadata().into()
		}
	}

	impl block_builder_api::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(block: Block, data: InherentData) -> CheckInherentsResult {
			data.check_extrinsics(&block)
		}

		fn random_seed() -> <Block as BlockT>::Hash {
			RandomnessCollectiveFlip::random_seed()
		}
	}

	impl client_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(tx: <Block as BlockT>::Extrinsic) -> TransactionValidity {
			Executive::validate_transaction(tx)
		}
	}

	impl offchain_primitives::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(number: sr_primitives::traits::NumberFor<Block>) {
			Executive::offchain_worker(number)
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
		fn parachain_status(id: parachain::Id) -> Option<parachain::Status> {
			Parachains::parachain_status(&id)
		}
		fn parachain_code(id: parachain::Id) -> Option<Vec<u8>> {
			Parachains::parachain_code(&id)
		}
		fn ingress(to: parachain::Id, since: Option<BlockNumber>)
			-> Option<parachain::StructuredUnroutedIngress>
		{
			Parachains::ingress(to, since).map(parachain::StructuredUnroutedIngress)
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> Vec<(GrandpaId, u64)> {
			Grandpa::grandpa_authorities()
		}
	}

	impl babe_primitives::BabeApi<Block> for Runtime {
		fn configuration() -> babe_primitives::BabeConfiguration {
			// The choice of `c` parameter (where `1 - c` represents the
			// probability of a slot being empty), is done in accordance to the
			// slot duration and expected target block time, for safely
			// resisting network delays of maximum two seconds.
			// <https://research.web3.foundation/en/latest/polkadot/BABE/Babe/#6-practical-results>
			babe_primitives::BabeConfiguration {
				slot_duration: Babe::slot_duration(),
				epoch_length: EpochDuration::get(),
				c: PRIMARY_PROBABILITY,
				genesis_authorities: Babe::authorities(),
				randomness: Babe::randomness(),
				secondary_slots: true,
			}
		}
	}

	impl substrate_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			let seed = seed.as_ref().map(|s| rstd::str::from_utf8(&s).expect("Seed is an utf8 string"));
			SessionKeys::generate(seed)
		}
	}

	impl system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}
}
