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

//! The Polkadot runtime. This can be compiled with ``#[no_std]`, ready for Wasm.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit="256"]

mod curated_grandpa;
mod parachains;
mod claims;
mod slot_range;
mod slots;

use rstd::prelude::*;
use substrate_primitives::u32_trait::{_1, _2, _3, _4};
use primitives::{
	AccountId, AccountIndex, Balance, BlockNumber, Hash, Nonce, SessionKey, Signature,
	parachain, AuraId
};
use client::{
	block_builder::api::{self as block_builder_api, InherentData, CheckInherentsResult},
	runtime_api as client_api, impl_runtime_apis,
};
use sr_primitives::{
	ApplyResult, generic, transaction_validity::TransactionValidity, create_runtime_str, key_types,
	traits::{BlakeTwo256, Block as BlockT, DigestFor, StaticLookup, Convert}, impl_opaque_keys
};
use version::RuntimeVersion;
use grandpa::{AuthorityId as GrandpaId, fg_primitives::{self, ScheduledChange}};
use elections::VoteIndex;
#[cfg(any(feature = "std", test))]
use version::NativeVersion;
use substrate_primitives::OpaqueMetadata;
use srml_support::{
	parameter_types, construct_runtime, traits::{SplitTwoWays, Currency, OnUnbalanced}
};

#[cfg(feature = "std")]
pub use staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use sr_primitives::BuildStorage;
pub use timestamp::Call as TimestampCall;
pub use balances::Call as BalancesCall;
pub use parachains::{Call as ParachainsCall, INHERENT_IDENTIFIER as PARACHAIN_INHERENT_IDENTIFIER};
pub use sr_primitives::{Permill, Perbill};
pub use srml_support::StorageValue;

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/// Runtime version.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("polkadot"),
	impl_name: create_runtime_str!("parity-polkadot"),
	authoring_version: 1,
	spec_version: 1000,
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

const DOTS: Balance = 1_000_000_000_000;
const BUCKS: Balance = DOTS / 100;
const CENTS: Balance = BUCKS / 100;
const MILLICENTS: Balance = CENTS / 1_000;

const SECS_PER_BLOCK: BlockNumber = 6;
const MINUTES: BlockNumber = 60 / SECS_PER_BLOCK;
const HOURS: BlockNumber = MINUTES * 60;
const DAYS: BlockNumber = HOURS * 24;

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl system::Trait for Runtime {
	type Origin = Origin;
	type Index = Nonce;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = Indices;
	type Header = generic::Header<BlockNumber, BlakeTwo256>;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
}

impl aura::Trait for Runtime {
	type HandleReport = aura::StakingSlasher<Runtime>;
	type AuthorityId = AuraId;
}

impl indices::Trait for Runtime {
	type IsDeadAccount = Balances;
	type AccountIndex = AccountIndex;
	type ResolveHint = indices::SimpleResolveHint<Self::AccountId, Self::AccountIndex>;
	type Event = Event;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 1 * BUCKS;
	pub const TransferFee: Balance = 1 * CENTS;
	pub const CreationFee: Balance = 1 * CENTS;
	pub const TransactionBaseFee: Balance = 1 * CENTS;
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
}


/// Logic for the author to get a portion of fees.
pub struct ToAuthor;

type NegativeImbalance = <Balances as Currency<AccountId>>::NegativeImbalance;
impl OnUnbalanced<NegativeImbalance> for ToAuthor {
	fn on_unbalanced(amount: NegativeImbalance) {
		Balances::resolve_creating(&Authorship::author(), amount);
	}
}

/// Splits fees 80/20 between treasury and block author.
pub type DealWithFees = SplitTwoWays<
	Balance,
	NegativeImbalance,
	_4, Treasury,   // 4 parts (80%) goes to the treasury.
	_1, ToAuthor,     // 1 part (20%) goes to the block author.
>;

impl balances::Trait for Runtime {
	type Balance = Balance;
	type OnFreeBalanceZero = Staking;
	type OnNewAccount = Indices;
	type Event = Event;
	type TransactionPayment = DealWithFees;
	type DustRemoval = ();
	type TransferPayment = ();
	type ExistentialDeposit = ExistentialDeposit;
	type TransferFee = TransferFee;
	type CreationFee = CreationFee;
	type TransactionBaseFee = TransactionBaseFee;
	type TransactionByteFee = TransactionByteFee;
}

parameter_types! {
	pub const MinimumPeriod: u64 = SECS_PER_BLOCK / 2;
}

impl timestamp::Trait for Runtime {
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = MinimumPeriod;
}

parameter_types! {
	pub const UncleGenerations: u64 = 0;
}

// TODO: substrate#2986 implement this properly
impl authorship::Trait for Runtime {
	type FindAuthor = ();
	type UncleGenerations = UncleGenerations;
	type FilterUncle = ();
	type EventHandler = ();
}

parameter_types! {
	pub const Period: BlockNumber = 10 * MINUTES;
	pub const Offset: BlockNumber = 0;
}

type SessionHandlers = (Grandpa, Aura);
impl_opaque_keys! {
	pub struct SessionKeys {
		#[id(key_types::ED25519)]
		pub ed25519: GrandpaId,
	}
}

// NOTE: `SessionHandler` and `SessionKeys` are co-dependent: One key will be used for each handler.
// The number and order of items in `SessionHandler` *MUST* be the same number and order of keys in
// `SessionKeys`.
// TODO: Introduce some structure to tie these together to make it a bit less of a footgun. This
// should be easy, since OneSessionHandler trait provides the `Key` as an associated type. #2858

impl session::Trait for Runtime {
	type OnSessionEnding = Staking;
	type SessionHandler = SessionHandlers;
	type ShouldEndSession = session::PeriodicSessions<Period, Offset>;
	type Event = Event;
	type Keys = SessionKeys;
	type SelectInitialValidators = Staking;
	type ValidatorId = AccountId;
	type ValidatorIdOf = staking::StashOf<Self>;
}

impl session::historical::Trait for Runtime {
	type FullIdentification = staking::Exposure<AccountId, Balance>;
	type FullIdentificationOf = staking::ExposureOf<Self>;
}

/// Converter for currencies to votes.
pub struct CurrencyToVoteHandler;

impl CurrencyToVoteHandler {
	fn factor() -> u128 { (Balances::total_issuance() / u64::max_value() as u128).max(1) }
}

impl Convert<u128, u64> for CurrencyToVoteHandler {
	fn convert(x: u128) -> u64 { (x / Self::factor()) as u64 }
}

impl Convert<u128, u128> for CurrencyToVoteHandler {
	fn convert(x: u128) -> u128 { x * Self::factor() }
}

parameter_types! {
	pub const SessionsPerEra: session::SessionIndex = 6;
	pub const BondingDuration: staking::EraIndex = 24 * 28;
}

impl staking::Trait for Runtime {
	type OnRewardMinted = Treasury;
	type CurrencyToVote = CurrencyToVoteHandler;
	type Event = Event;
	type Currency = Balances;
	type Slash = ();
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SessionInterface = Self;
}

parameter_types! {
	pub const LaunchPeriod: BlockNumber = 28 * 24 * 60 * MINUTES;
	pub const VotingPeriod: BlockNumber = 28 * 24 * 60 * MINUTES;
	pub const EmergencyVotingPeriod: BlockNumber = 3 * 24 * 60 * MINUTES;
	pub const MinimumDeposit: Balance = 100 * BUCKS;
	pub const EnactmentPeriod: BlockNumber = 30 * 24 * 60 * MINUTES;
	pub const CooloffPeriod: BlockNumber = 30 * 24 * 60 * MINUTES;
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
	type ExternalOrigin = collective::EnsureProportionAtLeast<_1, _2, AccountId, CouncilInstance>;
	type ExternalMajorityOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilInstance>;
	type ExternalPushOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, TechnicalInstance>;
	type EmergencyOrigin = collective::EnsureProportionAtLeast<_1, _1, AccountId, CouncilInstance>;
	type CancellationOrigin = collective::EnsureProportionAtLeast<_2, _3, AccountId, CouncilInstance>;
	type VetoOrigin = collective::EnsureMember<AccountId, CouncilInstance>;
	type CooloffPeriod = CooloffPeriod;
}

type CouncilInstance = collective::Instance1;
impl collective::Trait<CouncilInstance> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
}

parameter_types! {
	pub const CandidacyBond: Balance = 10 * BUCKS;
	pub const VotingBond: Balance = 1 * BUCKS;
	pub const VotingFee: Balance = 2 * BUCKS;
	pub const PresentSlashPerVoter: Balance = 1 * CENTS;
	pub const CarryCount: u32 = 6;
	// one additional vote should go by before an inactive voter can be reaped.
	pub const InactiveGracePeriod: VoteIndex = 1;
	pub const ElectionsVotingPeriod: BlockNumber = 2 * DAYS;
	pub const DecayRatio: u32 = 0;
}

impl elections::Trait for Runtime {
	type Event = Event;
	type Currency = Balances;
	type BadPresentation = ();
	type BadReaper = ();
	type BadVoterIndex = ();
	type LoserCandidate = ();
	type ChangeMembers = Council;
	type CandidacyBond = CandidacyBond;
	type VotingBond = VotingBond;
	type VotingFee = VotingFee;
	type PresentSlashPerVoter = PresentSlashPerVoter;
	type CarryCount = CarryCount;
	type InactiveGracePeriod = InactiveGracePeriod;
	type VotingPeriod = ElectionsVotingPeriod;
	type DecayRatio = DecayRatio;
}

type TechnicalInstance = collective::Instance2;
impl collective::Trait<TechnicalInstance> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
}

parameter_types! {
	pub const ProposalBond: Permill = Permill::from_percent(5);
	pub const ProposalBondMinimum: Balance = 1 * BUCKS;
	pub const SpendPeriod: BlockNumber = 1 * DAYS;
	pub const Burn: Permill = Permill::from_percent(50);
}

impl treasury::Trait for Runtime {
	type Currency = Balances;
	type ApproveOrigin = collective::EnsureMembers<_4, AccountId, CouncilInstance>;
	type RejectOrigin = collective::EnsureMembers<_2, AccountId, CouncilInstance>;
	type Event = Event;
	type MintedForSpending = ();
	type ProposalRejection = ();
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type SpendPeriod = SpendPeriod;
	type Burn = Burn;
}

impl grandpa::Trait for Runtime {
	type Event = Event;
}

parameter_types! {
	pub const WindowSize: BlockNumber = finality_tracker::DEFAULT_WINDOW_SIZE.into();
	pub const ReportLatency: BlockNumber = finality_tracker::DEFAULT_REPORT_LATENCY.into();
}

impl finality_tracker::Trait for Runtime {
	type OnFinalizationStalled = Grandpa;
	type WindowSize = WindowSize;
	type ReportLatency = ReportLatency;
}

impl parachains::Trait for Runtime {
	type Origin = Origin;
	type Call = Call;
	type ParachainCurrency = Balances;
}

parameter_types!{
	pub const LeasePeriod: BlockNumber = 100_000;
	pub const EndingPeriod: BlockNumber = 1000;
}

impl slots::Trait for Runtime {
	type Event = Event;
	type Currency = balances::Module<Self>;
	type Parachains = parachains::Module<Self>;
	type LeasePeriod = LeasePeriod;
	type EndingPeriod = EndingPeriod;
}

impl curated_grandpa::Trait for Runtime { }

impl sudo::Trait for Runtime {
	type Event = Event;
	type Proposal = Call;
}

construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = primitives::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: system::{Module, Call, Storage, Config, Event},
		Aura: aura::{Module, Config<T>, Inherent(Timestamp)},
		Timestamp: timestamp::{Module, Call, Storage, Inherent},
		Authorship: authorship::{Module, Call, Storage},
		Indices: indices,
		Balances: balances,
		Staking: staking::{default, OfflineWorker},
		Session: session::{Module, Call, Storage, Event, Config<T>},
		Democracy: democracy::{Module, Call, Storage, Config, Event<T>},
		Council: collective::<Instance1>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>},
		TechnicalCommittee: collective::<Instance2>::{Module, Call, Storage, Origin<T>, Event<T>, Config<T>},
		Elections: elections::{Module, Call, Storage, Event<T>, Config<T>},
		FinalityTracker: finality_tracker::{Module, Call, Inherent},
		Grandpa: grandpa::{Module, Call, Storage, Config, Event},
		CuratedGrandpa: curated_grandpa::{Module, Call, Config<T>, Storage},
		Treasury: treasury::{Module, Call, Storage, Event<T>},
		Parachains: parachains::{Module, Call, Storage, Config<T>, Inherent, Origin},
		Slots: slots::{Module, Call, Storage, Event<T>},
		Sudo: sudo,
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
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedMortalCompactExtrinsic<Address, Nonce, Call, Signature>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Nonce, Call>;
/// Executive: handles dispatch to the various modules.
pub type Executive = executive::Executive<Runtime, Block, system::ChainContext<Runtime>, Balances, Runtime, AllModules>;

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
			System::random_seed()
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
			Aura::authorities()  // only possible as long as parachain validator crypto === aura crypto
		}
		fn duty_roster() -> parachain::DutyRoster {
			Parachains::calculate_duty_roster()
		}
		fn active_parachains() -> Vec<parachain::Id> {
			Parachains::active_parachains()
		}
		fn parachain_status(id: parachain::Id) -> Option<parachain::Status> {
			Parachains::parachain_status(&id)
		}
		fn parachain_code(id: parachain::Id) -> Option<Vec<u8>> {
			Parachains::parachain_code(&id)
		}
		fn ingress(to: parachain::Id) -> Option<parachain::StructuredUnroutedIngress> {
			Parachains::ingress(to).map(parachain::StructuredUnroutedIngress)
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_pending_change(digest: &DigestFor<Block>)
			-> Option<ScheduledChange<BlockNumber>>
		{
			Grandpa::pending_change(digest)
		}

		fn grandpa_forced_change(_digest: &DigestFor<Block>)
			-> Option<(BlockNumber, ScheduledChange<BlockNumber>)>
		{
			None // disable forced changes.
		}

		fn grandpa_authorities() -> Vec<(SessionKey, u64)> {
			Grandpa::grandpa_authorities()
		}
	}

	impl consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> u64 {
			Aura::slot_duration()
		}

		fn authorities() -> Vec<AuraId> {
			Aura::authorities()
		}
	}

}
