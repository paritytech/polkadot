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

use rstd::prelude::*;
use codec::{Encode, Decode};
use primitives::{
	AccountId, AccountIndex, Balance, BlockNumber, Hash as HashT, Nonce, Signature, Moment,
	parachain::{self, ActiveParas, AbridgedCandidateReceipt, SigningContext}, ValidityError,
};
use runtime_common::{attestations, claims, parachains, registrar, slots,
	impls::{CurrencyToVoteHandler, TargetedFeeAdjustment},
	BlockHashCount, MaximumBlockWeight, AvailableBlockRatio,
	MaximumBlockLength,
};

use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	ApplyExtrinsicResult, Perbill, RuntimeDebug, KeyTypeId,
	transaction_validity::{
		TransactionValidity, InvalidTransaction, TransactionValidityError, TransactionSource, TransactionPriority,
	},
	curve::PiecewiseLinear,
	traits::{
		BlakeTwo256, Block as BlockT, StaticLookup, SignedExtension, OpaqueKeys, ConvertInto,
		DispatchInfoOf,
	},
};
use version::RuntimeVersion;
use grandpa::{AuthorityId as GrandpaId, fg_primitives};
#[cfg(any(feature = "std", test))]
use version::NativeVersion;
use sp_core::OpaqueMetadata;
use sp_staking::SessionIndex;
use frame_support::{
	parameter_types, construct_runtime,
	traits::{KeyOwnerProofSystem, Randomness},
};
use pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo;
use session::historical as session_historical;

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

/// Runtime version (Kusama).
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("polkadot-test-runtime"),
	impl_name: create_runtime_str!("parity-polkadot-test-runtime"),
	authoring_version: 2,
	spec_version: 1049,
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

/// Avoid processing transactions from slots and parachain registrar.
#[derive(Default, Encode, Decode, Clone, Eq, PartialEq, RuntimeDebug)]
pub struct RestrictFunctionality;
impl SignedExtension for RestrictFunctionality {
	const IDENTIFIER: &'static str = "RestrictFunctionality";
	type AccountId = AccountId;
	type Call = Call;
	type AdditionalSigned = ();
	type Pre = ();

	fn additional_signed(&self) -> rstd::result::Result<(), TransactionValidityError> { Ok(()) }

	fn validate(
		&self,
		_: &Self::AccountId,
		call: &Self::Call,
		_: &DispatchInfoOf<Self::Call>,
		_: usize
	)
		-> TransactionValidity
	{
		match call {
			Call::Slots(_) | Call::Registrar(_)
				=> Err(InvalidTransaction::Custom(ValidityError::NoPermission.into()).into()),
			_ => Ok(Default::default()),
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
	type Hash = HashT;
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
	type ModuleToIndex = ModuleToIndex;
	type AccountData = balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
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
	pub const IndexDeposit: Balance = 1 * DOLLARS;
}

impl indices::Trait for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type Event = Event;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 1 * CENTS;
}

impl balances::Trait for Runtime {
	type Balance = Balance;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
}

parameter_types! {
	pub const TransactionBaseFee: Balance = 1 * CENTS;
	pub const TransactionByteFee: Balance = 10 * MILLICENTS;
	// for a sane configuration, this should always be less than `AvailableBlockRatio`.
	pub const TargetBlockFullness: Perbill = Perbill::from_percent(25);
}

impl transaction_payment::Trait for Runtime {
	type Currency = Balances;
	type OnTransactionPayment = ();
	type TransactionBaseFee = TransactionBaseFee;
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
	type EventHandler = Staking;
}

parameter_types! {
	pub const Period: BlockNumber = 10 * MINUTES;
	pub const Offset: BlockNumber = 0;
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub parachain_validator: Parachains,
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
	type SessionManager = Staking;
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
	// Six sessions in an era (6 hours).
	pub const SessionsPerEra: SessionIndex = 6;
	// 28 eras for unbonding (7 days).
	pub const BondingDuration: staking::EraIndex = 28;
	// 28 eras in which slashes can be cancelled (7 days).
	pub const SlashDeferDuration: staking::EraIndex = 28;
	pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	pub const MaxNominatorRewardedPerValidator: u32 = 64;
	pub const ElectionLookahead: BlockNumber = 0;
	pub const StakingUnsignedPriority: TransactionPriority = TransactionPriority::max_value() / 2;
}

impl staking::Trait for Runtime {
	type Currency = Balances;
	type UnixTime = Timestamp;
	type CurrencyToVote = CurrencyToVoteHandler<Self>;
	type RewardRemainder = ();
	type Event = Event;
	type Slash = ();
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SlashDeferDuration = SlashDeferDuration;
	// A majority of the council can cancel the slash.
	type SlashCancelOrigin = system::EnsureNever<()>;
	type SessionInterface = Self;
	type RewardCurve = RewardCurve;
	type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
	type NextNewSession = Session;
	type ElectionLookahead = ElectionLookahead;
	type Call = Call;
	type SubmitTransaction = system::offchain::TransactionSubmitter<(), Runtime, Extrinsic>;
	type UnsignedPriority = StakingUnsignedPriority;
}

impl grandpa::Trait for Runtime {
	type Event = Event;
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

	pub const ValidationUpgradeFrequency: BlockNumber = 2;
	pub const ValidationUpgradeDelay: BlockNumber = 1;
	pub const SlashPeriod: BlockNumber = 1 * MINUTES;
}

impl parachains::Trait for Runtime {
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

	type Proof = session::historical::Proof;
	type KeyOwnerProofSystem = session::historical::Module<Self>;
	type IdentificationTuple = <
			Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, Vec<u8>)>
		>::IdentificationTuple;
	type ReportOffence = Offences;
	type BlockHashConversion = sp_runtime::traits::Identity;
}

impl offences::Trait for Runtime {
	type Event = Event;
	type IdentificationTuple = session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = Staking;
}

parameter_types! {
	pub const ParathreadDeposit: Balance = 5 * DOLLARS;
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
	type LeasePeriod = LeasePeriod;
	type EndingPeriod = EndingPeriod;
	type Randomness = RandomnessCollectiveFlip;
}

parameter_types! {
	pub const Prefix: &'static [u8] = b"Pay KSMs to the Kusama account:";
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

construct_runtime! {
	pub enum Runtime where
		Block = Block,
		NodeBlock = primitives::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		// Basic stuff; balances is uncallable initially.
		System: system::{Module, Call, Storage, Config, Event<T>},
		RandomnessCollectiveFlip: randomness_collective_flip::{Module, Storage},

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
		Grandpa: grandpa::{Module, Call, Storage, Config, Event},

		// Claims. Usable initially.
		Claims: claims::{Module, Call, Storage, Event<T>, Config<T>, ValidateUnsigned},

		// Parachains stuff; slots are disabled (no auctions initially). The rest are safe as they
		// have no public dispatchables.
		Parachains: parachains::{Module, Call, Storage, Config, Inherent, Origin},
		Attestations: attestations::{Module, Call, Storage},
		Slots: slots::{Module, Call, Storage, Event<T>},
		Registrar: registrar::{Module, Call, Storage, Event, Config<T>},

		// Vesting. Usable initially, but removed once all vesting is finished.
		Vesting: vesting::{Module, Call, Storage, Event<T>, Config<T>},
	}
}

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
	RestrictFunctionality,
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

pub type Hash = <Block as BlockT>::Hash;
pub type Extrinsic = <Block as BlockT>::Extrinsic;

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

		fn current_epoch_start() -> babe_primitives::SlotNumber {
			Babe::current_epoch_start()
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

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<
		Block,
		Balance,
		UncheckedExtrinsic,
	> for Runtime {
		fn query_info(uxt: UncheckedExtrinsic, len: u32) -> RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
	}
}
