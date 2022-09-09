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

//! Mocks for all the traits.

use crate::{
	configuration, disputes, dmp, hrmp, inclusion, initializer, origin, paras, paras_inherent,
	scheduler, session_info, shared,
	ump::{self, MessageId, UmpSink},
	ParaId,
};

use frame_support::{
	parameter_types,
	traits::{GenesisBuild, KeyOwnerProofSystem, ValidatorSet, ValidatorSetWithIdentification},
	weights::Weight,
};
use frame_support_test::TestRandomness;
use parity_scale_codec::Decode;
use primitives::v2::{
	AuthorityDiscoveryId, Balance, BlockNumber, Header, Moment, SessionIndex, UpwardMessage,
	ValidatorIndex,
};
use sp_core::H256;
use sp_io::TestExternalities;
use sp_runtime::{
	traits::{BlakeTwo256, IdentityLookup},
	transaction_validity::TransactionPriority,
	KeyTypeId, Permill,
};
use std::{cell::RefCell, collections::HashMap};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system,
		Balances: pallet_balances,
		Paras: paras,
		Configuration: configuration,
		ParasShared: shared,
		ParaInclusion: inclusion,
		ParaInherent: paras_inherent,
		Scheduler: scheduler,
		Initializer: initializer,
		Dmp: dmp,
		Ump: ump,
		Hrmp: hrmp,
		ParachainsOrigin: origin,
		SessionInfo: session_info,
		Disputes: disputes,
		Babe: pallet_babe,
	}
);

impl<C> frame_system::offchain::SendTransactionTypes<C> for Test
where
	Call: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = Call;
}

parameter_types! {
	pub const BlockHashCount: u32 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(Weight::from_ref_time(4 * 1024 * 1024));
}

pub type AccountId = u64;

impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = BlockWeights;
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = BlockNumber;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<u64>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub static ExistentialDeposit: u64 = 0;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

parameter_types! {
	pub const EpochDuration: u64 = 10;
	pub const ExpectedBlockTime: Moment = 6_000;
	pub const ReportLongevity: u64 = 10;
	pub const MaxAuthorities: u32 = 100_000;
}

impl pallet_babe::Config for Test {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;

	// session module is the trigger
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;

	type DisabledValidators = ();

	type KeyOwnerProof = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::Proof;

	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::IdentificationTuple;

	type KeyOwnerProofSystem = ();

	type HandleEquivocation = ();

	type WeightInfo = ();

	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const MinimumPeriod: Moment = 6_000 / 2;
}

impl pallet_timestamp::Config for Test {
	type Moment = Moment;
	type OnTimestampSet = ();
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
}

impl crate::initializer::Config for Test {
	type Randomness = TestRandomness<Self>;
	type ForceOrigin = frame_system::EnsureRoot<u64>;
	type WeightInfo = ();
}

impl crate::configuration::Config for Test {
	type WeightInfo = crate::configuration::TestWeightInfo;
}

impl crate::shared::Config for Test {}

impl origin::Config for Test {}

parameter_types! {
	pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

/// A very dumb implementation of `EstimateNextSessionRotation`. At the moment of writing, this
/// is more to satisfy type requirements rather than to test anything.
pub struct TestNextSessionRotation;

impl frame_support::traits::EstimateNextSessionRotation<u32> for TestNextSessionRotation {
	fn average_session_length() -> u32 {
		10
	}

	fn estimate_current_session_progress(_now: u32) -> (Option<Permill>, Weight) {
		(None, Weight::zero())
	}

	fn estimate_next_session_rotation(_now: u32) -> (Option<u32>, Weight) {
		(None, Weight::zero())
	}
}

impl crate::paras::Config for Test {
	type Event = Event;
	type WeightInfo = crate::paras::TestWeightInfo;
	type UnsignedPriority = ParasUnsignedPriority;
	type NextSessionRotation = TestNextSessionRotation;
}

impl crate::dmp::Config for Test {}

parameter_types! {
	pub const FirstMessageFactorPercent: u64 = 100;
}

impl crate::ump::Config for Test {
	type Event = Event;
	type UmpSink = TestUmpSink;
	type FirstMessageFactorPercent = FirstMessageFactorPercent;
	type ExecuteOverweightOrigin = frame_system::EnsureRoot<AccountId>;
	type WeightInfo = crate::ump::TestWeightInfo;
}

impl crate::hrmp::Config for Test {
	type Event = Event;
	type Origin = Origin;
	type Currency = pallet_balances::Pallet<Test>;
	type WeightInfo = crate::hrmp::TestWeightInfo;
}

impl crate::disputes::Config for Test {
	type Event = Event;
	type RewardValidators = Self;
	type PunishValidators = Self;
	type WeightInfo = crate::disputes::TestWeightInfo;
}

thread_local! {
	pub static REWARD_VALIDATORS: RefCell<Vec<(SessionIndex, Vec<ValidatorIndex>)>> = RefCell::new(Vec::new());
	pub static PUNISH_VALIDATORS_FOR: RefCell<Vec<(SessionIndex, Vec<ValidatorIndex>)>> = RefCell::new(Vec::new());
	pub static PUNISH_VALIDATORS_AGAINST: RefCell<Vec<(SessionIndex, Vec<ValidatorIndex>)>> = RefCell::new(Vec::new());
	pub static PUNISH_VALIDATORS_INCONCLUSIVE: RefCell<Vec<(SessionIndex, Vec<ValidatorIndex>)>> = RefCell::new(Vec::new());
}

impl crate::disputes::RewardValidators for Test {
	fn reward_dispute_statement(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		REWARD_VALIDATORS.with(|r| r.borrow_mut().push((session, validators.into_iter().collect())))
	}
}

impl crate::disputes::PunishValidators for Test {
	fn punish_for_invalid(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		PUNISH_VALIDATORS_FOR
			.with(|r| r.borrow_mut().push((session, validators.into_iter().collect())))
	}

	fn punish_against_valid(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		PUNISH_VALIDATORS_AGAINST
			.with(|r| r.borrow_mut().push((session, validators.into_iter().collect())))
	}

	fn punish_inconclusive(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		PUNISH_VALIDATORS_INCONCLUSIVE
			.with(|r| r.borrow_mut().push((session, validators.into_iter().collect())))
	}
}

impl crate::scheduler::Config for Test {}

impl crate::inclusion::Config for Test {
	type Event = Event;
	type DisputesHandler = Disputes;
	type RewardValidators = TestRewardValidators;
}

impl crate::paras_inherent::Config for Test {
	type WeightInfo = crate::paras_inherent::TestWeightInfo;
}

pub struct MockValidatorSet;

impl ValidatorSet<AccountId> for MockValidatorSet {
	type ValidatorId = AccountId;
	type ValidatorIdOf = ValidatorIdOf;
	fn session_index() -> SessionIndex {
		0
	}
	fn validators() -> Vec<Self::ValidatorId> {
		Vec::new()
	}
}

impl ValidatorSetWithIdentification<AccountId> for MockValidatorSet {
	type Identification = ();
	type IdentificationOf = FoolIdentificationOf;
}

pub struct FoolIdentificationOf;
impl sp_runtime::traits::Convert<AccountId, Option<()>> for FoolIdentificationOf {
	fn convert(_: AccountId) -> Option<()> {
		Some(())
	}
}

pub struct ValidatorIdOf;
impl sp_runtime::traits::Convert<AccountId, Option<AccountId>> for ValidatorIdOf {
	fn convert(a: AccountId) -> Option<AccountId> {
		Some(a)
	}
}

impl crate::session_info::Config for Test {
	type ValidatorSet = MockValidatorSet;
}

thread_local! {
	pub static DISCOVERY_AUTHORITIES: RefCell<Vec<AuthorityDiscoveryId>> = RefCell::new(Vec::new());
}

pub fn discovery_authorities() -> Vec<AuthorityDiscoveryId> {
	DISCOVERY_AUTHORITIES.with(|r| r.borrow().clone())
}

pub fn set_discovery_authorities(new: Vec<AuthorityDiscoveryId>) {
	DISCOVERY_AUTHORITIES.with(|r| *r.borrow_mut() = new);
}

impl crate::session_info::AuthorityDiscoveryConfig for Test {
	fn authorities() -> Vec<AuthorityDiscoveryId> {
		discovery_authorities()
	}
}

thread_local! {
	pub static BACKING_REWARDS: RefCell<HashMap<ValidatorIndex, usize>>
		= RefCell::new(HashMap::new());

	pub static AVAILABILITY_REWARDS: RefCell<HashMap<ValidatorIndex, usize>>
		= RefCell::new(HashMap::new());
}

pub fn backing_rewards() -> HashMap<ValidatorIndex, usize> {
	BACKING_REWARDS.with(|r| r.borrow().clone())
}

pub fn availability_rewards() -> HashMap<ValidatorIndex, usize> {
	AVAILABILITY_REWARDS.with(|r| r.borrow().clone())
}

std::thread_local! {
	static PROCESSED: RefCell<Vec<(ParaId, UpwardMessage)>> = RefCell::new(vec![]);
}

/// Return which messages have been processed by `pocess_upward_message` and clear the buffer.
pub fn take_processed() -> Vec<(ParaId, UpwardMessage)> {
	PROCESSED.with(|opt_hook| std::mem::take(&mut *opt_hook.borrow_mut()))
}

/// An implementation of a UMP sink that just records which messages were processed.
///
/// A message's weight is defined by the first 4 bytes of its data, which we decode into a
/// `u32`.
pub struct TestUmpSink;
impl UmpSink for TestUmpSink {
	fn process_upward_message(
		actual_origin: ParaId,
		actual_msg: &[u8],
		max_weight: Weight,
	) -> Result<Weight, (MessageId, Weight)> {
		let weight = match u32::decode(&mut &actual_msg[..]) {
			Ok(w) => Weight::from_ref_time(w as u64),
			Err(_) => return Ok(Weight::zero()), // same as the real `UmpSink`
		};
		if weight.any_gt(max_weight) {
			let id = sp_io::hashing::blake2_256(actual_msg);
			return Err((id, weight))
		}

		PROCESSED.with(|opt_hook| {
			opt_hook.borrow_mut().push((actual_origin, actual_msg.to_owned()));
		});
		Ok(weight)
	}
}

pub struct TestRewardValidators;

impl inclusion::RewardValidators for TestRewardValidators {
	fn reward_backing(v: impl IntoIterator<Item = ValidatorIndex>) {
		BACKING_REWARDS.with(|r| {
			let mut r = r.borrow_mut();
			for i in v {
				*r.entry(i).or_insert(0) += 1;
			}
		})
	}
	fn reward_bitfields(v: impl IntoIterator<Item = ValidatorIndex>) {
		AVAILABILITY_REWARDS.with(|r| {
			let mut r = r.borrow_mut();
			for i in v {
				*r.entry(i).or_insert(0) += 1;
			}
		})
	}
}

/// Create a new set of test externalities.
pub fn new_test_ext(state: MockGenesisConfig) -> TestExternalities {
	use sp_keystore::{testing::KeyStore, KeystoreExt, SyncCryptoStorePtr};
	use sp_std::sync::Arc;

	sp_tracing::try_init_simple();

	BACKING_REWARDS.with(|r| r.borrow_mut().clear());
	AVAILABILITY_REWARDS.with(|r| r.borrow_mut().clear());

	let mut t = state.system.build_storage::<Test>().unwrap();
	state.configuration.assimilate_storage(&mut t).unwrap();
	GenesisBuild::<Test>::assimilate_storage(&state.paras, &mut t).unwrap();

	let mut ext: TestExternalities = t.into();
	ext.register_extension(KeystoreExt(Arc::new(KeyStore::new()) as SyncCryptoStorePtr));

	ext
}

#[derive(Default)]
pub struct MockGenesisConfig {
	pub system: frame_system::GenesisConfig,
	pub configuration: crate::configuration::GenesisConfig<Test>,
	pub paras: crate::paras::GenesisConfig,
}

pub fn assert_last_event(generic_event: Event) {
	let events = frame_system::Pallet::<Test>::events();
	let system_event: <Test as frame_system::Config>::Event = generic_event.into();
	// compare to the last event record
	let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
	assert_eq!(event, &system_event);
}
