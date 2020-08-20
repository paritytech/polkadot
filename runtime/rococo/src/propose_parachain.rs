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

//! A pallet for proposing a parachain for Rococo.
//!
//! This pallet works as registration of parachains for Rococo. The idea is to have
//! the registration of community provides parachains being handled by this pallet.
//! People will be able to propose their parachain for registration. This proposal
//! will need to be improved by some priviledged account. After approval the workflow
//! is the following:
//!
//! 1. On start of the next session the pallet announces the new relay chain validators.
//!
//! 2. The session after announcing the new relay chain validators, they will be active. At the
//!    switch to this session, the parachain will be registered and is allowed to produce blocks.
//!
//! When deregistering a parachain, we basically reverse the operations.

use frame_support::{
	decl_event, decl_error, decl_module, traits::{Get, ReservableCurrency, EnsureOrigin, Currency},
	decl_storage, ensure, IterableStorageMap,
};
use primitives::v0::{Id as ParaId, Info as ParaInfo, Scheduling, HeadData, ValidationCode};
use polkadot_parachain::primitives::AccountIdConversion;
use system::{ensure_signed, EnsureOneOf, EnsureSigned};
use sp_runtime::Either;
use sp_staking::SessionIndex;
use sp_std::vec::Vec;
use runtime_common::registrar::Registrar;

type EnsurePriviledgedOrSigned<T> = EnsureOneOf<
	<T as system::Trait>::AccountId,
	<T as Trait>::PriviledgedOrigin,
	EnsureSigned<<T as system::Trait>::AccountId>
>;

type Session<T> = session::Module<T>;

type BalanceOf<T> = <<T as runtime_common::registrar::Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// Configuration for the parachain proposer.
pub trait Trait: session::Trait + runtime_common::parachains::Trait + runtime_common::registrar::Trait {
	/// The overreaching event type.
	type Event: From<Event> + Into<<Self as system::Trait>::Event>;

	/// The maximum name length of a parachain.
	type MaxNameLength: Get<u32>;

	/// The amount that should be deposited when creating a proposal.
	type ProposeDeposit: Get<BalanceOf<Self>>;

	/// Priviledged origin that can approve/cancel/deregister parachain and proposals.
	type PriviledgedOrigin: EnsureOrigin<<Self as system::Trait>::Origin>;
}

/// A proposal for adding a parachain to the relay chain.
#[derive(codec::Encode, codec::Decode)]
struct Proposal<AccountId, ValidatorId, Balance> {
	/// The account that proposed this parachain.
	proposer: AccountId,
	/// The validation WASM code of the parachain.
	validation_function: ValidationCode,
	/// The initial head state of the parachain.
	initial_head_state: HeadData,
	/// The validators for the relay chain provided by the parachain.
	validators: Vec<ValidatorId>,
	/// The name of the parachain.
	name: Vec<u8>,
	/// The balance that the parachain should receive.
	balance: Balance,
}

/// Information about the registered parachain.
#[derive(codec::Encode, codec::Decode)]
struct RegisteredParachainInfo<AccountId, ValidatorId> {
	/// The validators for the relay chain provided by the parachain.
	validators: Vec<ValidatorId>,
	/// The account that proposed the parachain.
	proposer: AccountId,
}

decl_event! {
	pub enum Event {
		/// A parachain was proposed for registration.
		ParachainProposed(Vec<u8>, ParaId),
		/// A parachain was approved and is scheduled for being activated.
		ParachainApproved(ParaId),
		/// A parachain was registered and is now running.
		ParachainRegistered(ParaId),
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// The name of the parachain is too long.
		NameTooLong,
		/// The requested parachain id is already registered.
		ParachainIdAlreadyTaken,
		/// The requested parachain id is already proposed for another parachain.
		ParachainIdAlreadyProposed,
		/// Could not find the parachain proposal.
		ProposalNotFound,
		/// Not authorized to do a certain operation.
		NotAuthorized,
		/// A validator is already registered in the active validator set.
		ValidatorAlreadyRegistered,
		/// No information about the registered parachain found.
		ParachainInfoNotFound,
		/// Parachain is already approved for registration.
		ParachainAlreadyApproved,
		/// Parachain is already scheduled for registration.
		ParachainAlreadyScheduled,
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as ParachainProposer {
		/// All the proposals.
		Proposals: map hasher(twox_64_concat) ParaId => Option<Proposal<T::AccountId, T::ValidatorId, BalanceOf<T>>>;
		/// Proposals that are approved.
		ApprovedProposals: Vec<ParaId>;
		/// Proposals that are scheduled at for a fixed session to be applied.
		ScheduledProposals: map hasher(twox_64_concat) SessionIndex => Vec<ParaId>;
		/// Information about the registered parachains.
		ParachainInfo: map hasher(twox_64_concat) ParaId => Option<RegisteredParachainInfo<T::AccountId, T::ValidatorId>>;
		/// Validators that should be retired, because their Parachain was deregistered.
		ValidatorsToRetire: Vec<T::ValidatorId>;
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin, system = system {
		type Error = Error<T>;

		/// The maximum name length of a parachain.
		const MaxNameLength: u32 = T::MaxNameLength::get();

		/// The deposit that will be reserved when proposing a parachain.
		const ProposeDeposit: BalanceOf<T> = T::ProposeDeposit::get();

		fn deposit_event() = default;

		/// Propose a new parachain
		///
		/// This requires:
		/// - `para_id`: The id of the parachain.
		/// - `name`: The name of the parachain.
		/// - `validation_function`: The wasm runtime of the parachain.
		/// - `initial_head_state`: The genesis state of the parachain.
		/// - `validators`: Two validators that will validate for the relay chain.
		/// - `balance`: The initial balance of the parachain on the relay chain.
		///
		/// It will reserve a deposit from the sender account over the lifetime of the chain.
		#[weight = 1000_000]
		fn propose_parachain(
			origin,
			para_id: ParaId,
			name: Vec<u8>,
			validation_function: ValidationCode,
			initial_head_state: HeadData,
			validators: [T::ValidatorId; 2],
			balance: BalanceOf<T>,
		) {
			let who = ensure_signed(origin)?;

			ensure!(name.len() <= T::MaxNameLength::get() as usize, Error::<T>::NameTooLong);
			ensure!(!Proposals::<T>::contains_key(&para_id), Error::<T>::ParachainIdAlreadyProposed);
			ensure!(!runtime_common::parachains::Code::contains_key(&para_id), Error::<T>::ParachainIdAlreadyTaken);

			let active_validators = Session::<T>::validators();
			ensure!(
				active_validators.iter().all(|v| *v != validators[0] && *v != validators[1]),
				Error::<T>::ValidatorAlreadyRegistered,
			);
			Proposals::<T>::iter().try_for_each(|(_, prop)|
				if prop.validators.iter().all(|v| *v != validators[0] && *v != validators[1]) {
					Ok(())
				} else {
					Err(Error::<T>::ValidatorAlreadyRegistered)
				}
			)?;

			T::Currency::reserve(&who, T::ProposeDeposit::get())?;

			let proposal = Proposal {
				name: name.clone(),
				proposer: who,
				validators: validators.into(),
				initial_head_state,
				validation_function,
				balance,
			};

			Proposals::<T>::insert(para_id, proposal);

			Self::deposit_event(Event::ParachainProposed(name, para_id));
		}

		/// Approve a parachain proposal.
		#[weight = 100_000]
		fn approve_proposal(
			origin,
			para_id: ParaId,
		) {
			T::PriviledgedOrigin::ensure_origin(origin)?;

			if !Proposals::<T>::contains_key(&para_id) {
				return Err(Error::<T>::ProposalNotFound.into())
			}

			Self::is_approved_or_scheduled(para_id)?;

			ApprovedProposals::append(para_id);

			Self::deposit_event(Event::ParachainApproved(para_id));
		}

		/// Cancel a parachain proposal.
		///
		/// This also unreserves the deposit.
		#[weight = 100_000]
		fn cancel_proposal(origin, para_id: ParaId) {
			let who = match EnsurePriviledgedOrSigned::<T>::ensure_origin(origin)? {
				Either::Left(_) => None,
				Either::Right(who) => Some(who),
			};

			Self::is_approved_or_scheduled(para_id)?;

			let proposal = Proposals::<T>::get(&para_id).ok_or(Error::<T>::ProposalNotFound)?;

			if let Some(who) = who {
				ensure!(who == proposal.proposer, Error::<T>::NotAuthorized);
			}

			Proposals::<T>::remove(&para_id);

			T::Currency::unreserve(&proposal.proposer, T::ProposeDeposit::get());
		}

		/// Deregister a parachain that was already successfully registered in the relay chain.
		#[weight = 100_000]
		fn deregister_parachain(origin, para_id: ParaId) {
			let who = match EnsurePriviledgedOrSigned::<T>::ensure_origin(origin)? {
				Either::Left(_) => None,
				Either::Right(who) => Some(who),
			};

			let info = ParachainInfo::<T>::get(&para_id).ok_or(Error::<T>::ParachainInfoNotFound)?;

			if let Some(who) = who {
				ensure!(who == info.proposer, Error::<T>::NotAuthorized);
			}

			ParachainInfo::<T>::remove(&para_id);
			info.validators.into_iter().for_each(|v| ValidatorsToRetire::<T>::append(v));
			let _ = T::Registrar::deregister_para(para_id);

			T::Currency::unreserve(&info.proposer, T::ProposeDeposit::get());
		}
	}
}

impl<T: Trait> Module<T> {
	/// Returns wether the given `para_id` approval is approved or already scheduled.
	fn is_approved_or_scheduled(para_id: ParaId) -> frame_support::dispatch::DispatchResult {
		if ApprovedProposals::get().iter().any(|p| *p == para_id) {
			return Err(Error::<T>::ParachainAlreadyApproved.into())
		}

		if ScheduledProposals::get(&Session::<T>::current_index() + 1).iter().any(|p| *p == para_id) {
			return Err(Error::<T>::ParachainAlreadyScheduled.into())
		}

		Ok(())
	}
}

impl<T: Trait> session::SessionManager<T::ValidatorId> for Module<T> {
	fn new_session(new_index: SessionIndex) -> Option<Vec<T::ValidatorId>> {
		if new_index <= 1 {
			return None;
		}

		let proposals = ApprovedProposals::take();

		let mut validators = Session::<T>::validators();

		ValidatorsToRetire::<T>::take().iter().for_each(|v| {
			if let Some(pos) = validators.iter().position(|r| r == v) {
				validators.swap_remove(pos);
			}
		});

		// Schedule all approved proposals
		for (id, proposal) in proposals.iter().filter_map(|id| Proposals::<T>::get(&id).map(|p| (id, p))) {
			ScheduledProposals::append(new_index, id);
			validators.extend(proposal.validators);
		}

		Some(validators)
	}

	fn end_session(_: SessionIndex) {}

	fn start_session(start_index: SessionIndex) {
		let proposals = ScheduledProposals::take(&start_index);

		// Register all parachains that are allowed to start with the new session.
		for (id, proposal) in proposals.iter().filter_map(|id| Proposals::<T>::take(&id).map(|p| (id, p))) {
			let info = ParaInfo {
				scheduling: Scheduling::Always,
			};

			// Ignore errors for now
			let _ = T::Registrar::register_para(
				*id,
				info,
				proposal.validation_function,
				proposal.initial_head_state,
			);
			Self::deposit_event(Event::ParachainRegistered(*id));

			// Add some funds to the Parachain
			let _ = T::Currency::deposit_creating(&id.into_account(), proposal.balance);

			let info = RegisteredParachainInfo {
				proposer: proposal.proposer,
				validators: proposal.validators,
			};
			ParachainInfo::<T>::insert(id, info);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_io::TestExternalities;
	use sp_core::H256;
	use sp_runtime::{
		traits::{
			BlakeTwo256, IdentityLookup, AccountIdConversion, Extrinsic as ExtrinsicT,
		}, testing::{UintAuthorityId, TestXt}, KeyTypeId, Perbill, curve::PiecewiseLinear,
	};
	use primitives::v0::{ValidatorId, HeadData, Balance, BlockNumber, Header, Signature};
	use frame_support::{
		traits::{KeyOwnerProofSystem, OnInitialize, OnFinalize},
		impl_outer_origin, impl_outer_dispatch, assert_ok, parameter_types, assert_noop,
	};
	use keyring::Sr25519Keyring;
	use runtime_common::{parachains, slots, attestations, registrar};

	impl_outer_origin! {
		pub enum Origin for Test where system = system {
			parachains,
		}
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			parachains::Parachains,
			registrar::Registrar,
			staking::Staking,
		}
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

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub const MaximumBlockWeight: u32 = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}
	impl system::Trait for Test {
		type BaseCallFilter = ();
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type DbWeight = ();
		type BlockExecutionWeight = ();
		type ExtrinsicBaseWeight = ();
		type MaximumExtrinsicWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
		type AccountData = balances::AccountData<u128>;
		type OnNewAccount = ();
		type OnKilledAccount = Balances;
		type SystemWeightInfo = ();
	}

	impl<C> system::offchain::SendTransactionTypes<C> for Test where
		Call: From<C>,
	{
		type OverarchingCall = Call;
		type Extrinsic = TestXt<Call, ()>;
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 1;
	}

	impl balances::Trait for Test {
		type Balance = u128;
		type DustRemoval = ();
		type Event = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type WeightInfo = ();
	}

	parameter_types!{
		pub const LeasePeriod: BlockNumber = 10;
		pub const EndingPeriod: BlockNumber = 3;
	}

	impl slots::Trait for Test {
		type Event = ();
		type Currency = balances::Module<Test>;
		type Parachains = Registrar;
		type EndingPeriod = EndingPeriod;
		type LeasePeriod = LeasePeriod;
		type Randomness = RandomnessCollectiveFlip;
	}

	parameter_types!{
		pub const SlashDeferDuration: staking::EraIndex = 7;
		pub const AttestationPeriod: BlockNumber = 100;
		pub const MinimumPeriod: u64 = 3;
		pub const SessionsPerEra: sp_staking::SessionIndex = 6;
		pub const BondingDuration: staking::EraIndex = 28;
		pub const MaxNominatorRewardedPerValidator: u32 = 64;
	}

	impl attestations::Trait for Test {
		type AttestationPeriod = AttestationPeriod;
		type ValidatorIdentities = parachains::ValidatorIdentities<Test>;
		type RewardAttestation = ();
	}

	parameter_types! {
		pub const Period: BlockNumber = 1;
		pub const Offset: BlockNumber = 0;
		pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
		pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	}

	impl session::Trait for Test {
		type SessionManager = ProposeParachain;
		type Keys = UintAuthorityId;
		type ShouldEndSession = session::PeriodicSessions<Period, Offset>;
		type NextSessionRotation = session::PeriodicSessions<Period, Offset>;
		type SessionHandler = session::TestSessionHandler;
		type Event = ();
		type ValidatorId = u64;
		type ValidatorIdOf = ();
		type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
		type WeightInfo = ();
	}

	parameter_types! {
		pub const MaxHeadDataSize: u32 = 100;
		pub const MaxCodeSize: u32 = 100;

		pub const ValidationUpgradeFrequency: BlockNumber = 10;
		pub const ValidationUpgradeDelay: BlockNumber = 2;
		pub const SlashPeriod: BlockNumber = 50;
		pub const ElectionLookahead: BlockNumber = 0;
		pub const StakingUnsignedPriority: u64 = u64::max_value() / 2;
	}

	impl staking::Trait for Test {
		type RewardRemainder = ();
		type CurrencyToVote = ();
		type Event = ();
		type Currency = balances::Module<Test>;
		type Slash = ();
		type Reward = ();
		type SessionsPerEra = SessionsPerEra;
		type BondingDuration = BondingDuration;
		type SlashDeferDuration = SlashDeferDuration;
		type SlashCancelOrigin = system::EnsureRoot<Self::AccountId>;
		type SessionInterface = Self;
		type UnixTime = timestamp::Module<Test>;
		type RewardCurve = RewardCurve;
		type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
		type NextNewSession = Session;
		type ElectionLookahead = ElectionLookahead;
		type Call = Call;
		type UnsignedPriority = StakingUnsignedPriority;
		type MaxIterations = ();
		type MinSolutionScoreBump = ();
		type WeightInfo = ();
	}

	impl timestamp::Trait for Test {
		type Moment = u64;
		type OnTimestampSet = ();
		type MinimumPeriod = MinimumPeriod;
		type WeightInfo = ();
	}

	impl session::historical::Trait for Test {
		type FullIdentification = staking::Exposure<u64, Balance>;
		type FullIdentificationOf = staking::ExposureOf<Self>;
	}

	// This is needed for a custom `AccountId` type which is `u64` in testing here.
	pub mod test_keys {
		use sp_core::{crypto::KeyTypeId, sr25519};
		use primitives::v0::Signature;

		pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"test");

		mod app {
			use super::super::Parachains;
			use sp_application_crypto::{app_crypto, sr25519};

			app_crypto!(sr25519, super::KEY_TYPE);

			impl sp_runtime::traits::IdentifyAccount for Public {
				type AccountId = u64;

				fn into_account(self) -> Self::AccountId {
					let id = self.0.clone().into();
					Parachains::authorities().iter().position(|b| *b == id).unwrap() as u64
				}
			}
		}

		pub type ReporterId = app::Public;
		pub struct ReporterAuthorityId;
		impl system::offchain::AppCrypto<ReporterId, Signature> for ReporterAuthorityId {
			type RuntimeAppPublic = ReporterId;
			type GenericSignature = sr25519::Signature;
			type GenericPublic = sr25519::Public;
		}
	}

	impl parachains::Trait for Test {
		type AuthorityId = test_keys::ReporterAuthorityId;
		type Origin = Origin;
		type Call = Call;
		type ParachainCurrency = balances::Module<Test>;
		type BlockNumberConversion = sp_runtime::traits::Identity;
		type ActiveParachains = Registrar;
		type Registrar = Registrar;
		type Randomness = RandomnessCollectiveFlip;
		type MaxCodeSize = MaxCodeSize;
		type MaxHeadDataSize = MaxHeadDataSize;
		type ValidationUpgradeFrequency = ValidationUpgradeFrequency;
		type ValidationUpgradeDelay = ValidationUpgradeDelay;
		type SlashPeriod = SlashPeriod;
		type Proof = sp_session::MembershipProof;
		type KeyOwnerProofSystem = session::historical::Module<Test>;
		type IdentificationTuple = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
			KeyTypeId,
			Vec<u8>,
		)>>::IdentificationTuple;
		type ReportOffence = ();
		type BlockHashConversion = sp_runtime::traits::Identity;
	}

	type Extrinsic = TestXt<Call, ()>;

	impl<LocalCall> system::offchain::CreateSignedTransaction<LocalCall> for Test where
		Call: From<LocalCall>,
	{
		fn create_transaction<C: system::offchain::AppCrypto<Self::Public, Self::Signature>>(
			call: Call,
			_public: test_keys::ReporterId,
			_account: <Test as system::Trait>::AccountId,
			nonce: <Test as system::Trait>::Index,
		) -> Option<(Call, <Extrinsic as ExtrinsicT>::SignaturePayload)> {
			Some((call, (nonce, ())))
		}
	}

	impl system::offchain::SigningTypes for Test {
		type Public = test_keys::ReporterId;
		type Signature = Signature;
	}

	parameter_types! {
		pub const ParathreadDeposit: Balance = 10;
		pub const QueueSize: usize = 2;
		pub const MaxRetries: u32 = 3;
	}

	impl registrar::Trait for Test {
		type Event = ();
		type Origin = Origin;
		type Currency = balances::Module<Test>;
		type ParathreadDeposit = ParathreadDeposit;
		type SwapAux = slots::Module<Test>;
		type QueueSize = QueueSize;
		type MaxRetries = MaxRetries;
	}

	parameter_types! {
		pub const ProposeDeposit: Balance = 10;
		pub const MaxNameLength: u32 = 10;
	}

	impl Trait for Test {
		type Event = ();
		type MaxNameLength = MaxNameLength;
		type ProposeDeposit = ProposeDeposit;
		type PriviledgedOrigin = system::EnsureRoot<u64>;
	}

	type Balances = balances::Module<Test>;
	type Parachains = parachains::Module<Test>;
	type System = system::Module<Test>;
	type Slots = slots::Module<Test>;
	type Registrar = registrar::Module<Test>;
	type RandomnessCollectiveFlip = randomness_collective_flip::Module<Test>;
	type Session = session::Module<Test>;
	type Staking = staking::Module<Test>;
	type ProposeParachain = Module<Test>;

	fn new_test_ext(parachains: Vec<(ParaId, ValidationCode, HeadData)>) -> TestExternalities {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();

		let authority_keys = [
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
			Sr25519Keyring::Two,
		];

		// stashes are the index.
		let session_keys: Vec<_> = authority_keys.iter().enumerate()
			.map(|(i, _k)| (i as u64, i as u64, UintAuthorityId(i as u64)))
			.collect();

		let authorities: Vec<_> = authority_keys.iter().map(|k| ValidatorId::from(k.public())).collect();

		let balances: Vec<_> = (0..authority_keys.len()).map(|i| (i as u64, 10_000_000)).collect();

		parachains::GenesisConfig {
			authorities,
		}.assimilate_storage::<Test>(&mut t).unwrap();

		registrar::GenesisConfig::<Test> {
			parachains,
			_phdata: Default::default(),
		}.assimilate_storage(&mut t).unwrap();

		session::GenesisConfig::<Test> {
			keys: session_keys,
		}.assimilate_storage(&mut t).unwrap();

		balances::GenesisConfig::<Test> {
			balances,
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	fn init_block() {
		println!("Initializing {}", System::block_number());
		System::on_initialize(System::block_number());
		Registrar::on_initialize(System::block_number());
		Parachains::on_initialize(System::block_number());
		Slots::on_initialize(System::block_number());
		Session::on_initialize(System::block_number());
		ProposeParachain::on_initialize(System::block_number());
	}

	fn run_to_block(n: BlockNumber) {
		println!("Running until block {}", n);
		while System::block_number() < n {
			if System::block_number() > 1 {
				println!("Finalizing {}", System::block_number());
				if !parachains::DidUpdate::exists() {
					println!("Null heads update");
					assert_ok!(Parachains::set_heads(system::RawOrigin::None.into(), vec![]));
				}
				Slots::on_finalize(System::block_number());
				Parachains::on_finalize(System::block_number());
				Registrar::on_finalize(System::block_number());
				Session::on_finalize(System::block_number());
				ProposeParachain::on_finalize(System::block_number());
				System::on_finalize(System::block_number());
			}
			System::set_block_number(System::block_number() + 1);
			init_block();
		}
	}

	#[test]
	fn propose_parachain_checks_name_length() {
		new_test_ext(Vec::new()).execute_with(|| {
			assert_noop!(
				ProposeParachain::propose_parachain(
					Origin::signed(10),
					10.into(),
					vec![0; 11],
					Default::default(),
					Default::default(),
					[11, 12],
					100,
				),
				Error::<Test>::NameTooLong,
			);
		})
	}

	#[test]
	fn propose_parachain_reserve_works() {
		new_test_ext(Vec::new()).execute_with(|| {
			// Fails because the account doesn't have enough funds
			assert_noop!(
				ProposeParachain::propose_parachain(
					Origin::signed(10),
					10.into(),
					vec![0; 10],
					Default::default(),
					Default::default(),
					[11, 12],
					100,
				),
				balances::Error::<Test, _>::InsufficientBalance,
			);

			let _ = Balances::deposit_creating(&10, 100);

			assert_eq!(
				ProposeParachain::propose_parachain(
					Origin::signed(10),
					10.into(),
					vec![0; 10],
					Default::default(),
					Default::default(),
					[11, 12],
					100,
				),
				Ok(())
			);

			assert_eq!(Balances::reserved_balance(&10), 10);
		})
	}

	#[test]
	fn propose_parachain_and_cancel_works() {
		new_test_ext(Vec::new()).execute_with(|| {
			let _ = Balances::deposit_creating(&10, 100);

			assert_eq!(
				ProposeParachain::propose_parachain(
					Origin::signed(10),
					10.into(),
					vec![0; 10],
					Default::default(),
					Default::default(),
					[11, 12],
					100,
				),
				Ok(())
			);

			assert_eq!(Balances::reserved_balance(&10), 10);

			assert_eq!(
				ProposeParachain::cancel_proposal(Origin::signed(10), 10.into()),
				Ok(())
			);

			assert_eq!(Balances::reserved_balance(&10), 0);

			assert_noop!(
				ProposeParachain::cancel_proposal(Origin::signed(10), 10.into()),
				Error::<Test>::ProposalNotFound,
			);
		})
	}

	#[test]
	fn proposal_workflow_works() {
		new_test_ext(Vec::new()).execute_with(|| {
			let _ = Balances::deposit_creating(&10, 100);

			let validation_code = ValidationCode(vec![0; 20]);

			assert_eq!(
				ProposeParachain::propose_parachain(
					Origin::signed(10),
					10.into(),
					vec![0; 10],
					validation_code.clone(),
					vec![0; 30].into(),
					[11, 12],
					100,
				),
				Ok(())
			);

			assert_eq!(Balances::reserved_balance(&10), 10);

			assert_eq!(ProposeParachain::approve_proposal(Origin::root(), 10.into()), Ok(()));
			assert_eq!(vec![ParaId::from(10)], ApprovedProposals::get());
			assert_noop!(ProposeParachain::approve_proposal(Origin::root(), 10.into()), Error::<Test>::ParachainAlreadyApproved);
			assert_noop!(ProposeParachain::cancel_proposal(Origin::root(), 10.into()), Error::<Test>::ParachainAlreadyApproved);

			run_to_block(1);
			assert!(ApprovedProposals::get().is_empty());
			assert_eq!(vec![ParaId::from(10)], ScheduledProposals::get(&2));
			assert_noop!(ProposeParachain::approve_proposal(Origin::root(), 10.into()), Error::<Test>::ParachainAlreadyScheduled);
			assert_noop!(ProposeParachain::cancel_proposal(Origin::root(), 10.into()), Error::<Test>::ParachainAlreadyScheduled);

			run_to_block(2);
			assert!(ScheduledProposals::get(&2).is_empty());

			assert_eq!(validation_code, parachains::Code::get(&ParaId::from(10)).unwrap());
			assert_eq!(Balances::free_balance(&ParaId::from(10).into_account()), 100);
			assert!(Proposals::<Test>::get(&ParaId::from(10)).is_none());
			assert_eq!(Balances::reserved_balance(&10), 10);
			assert!(Session::validators().iter().any(|v| *v == 11));
			assert!(Session::validators().iter().any(|v| *v == 12));

			assert_eq!(ProposeParachain::deregister_parachain(Origin::signed(10), 10.into()), Ok(()));
			assert_eq!(Balances::reserved_balance(&10), 0);
			assert_eq!(vec![11, 12], ValidatorsToRetire::<Test>::get());
			assert!(Session::validators().iter().any(|v| *v == 11));
			assert!(Session::validators().iter().any(|v| *v == 12));
			assert!(parachains::Code::get(&ParaId::from(10)).is_none());

			run_to_block(4);
			assert!(Session::validators().iter().all(|v| *v != 11 && *v != 12));
		})
	}

	#[test]
	fn propose_parachain_checks_validators() {
		new_test_ext(Vec::new()).execute_with(|| {
			let _ = Balances::deposit_creating(&10, 100);
			let _ = Balances::deposit_creating(&11, 100);

			assert_eq!(
				ProposeParachain::propose_parachain(
					Origin::signed(10),
					10.into(),
					vec![0; 10],
					Default::default(),
					Default::default(),
					[11, 12],
					100,
				),
				Ok(()),
			);

			assert_noop!(
				ProposeParachain::propose_parachain(
					Origin::signed(11),
					11.into(),
					vec![0; 10],
					Default::default(),
					Default::default(),
					[11, 13],
					100,
				),
				Error::<Test>::ValidatorAlreadyRegistered,
			);

			assert_noop!(
				ProposeParachain::propose_parachain(
					Origin::signed(11),
					11.into(),
					vec![0; 10],
					Default::default(),
					Default::default(),
					[5, 13],
					100,
				),
				Error::<Test>::ValidatorAlreadyRegistered,
			);
		})
	}
}
