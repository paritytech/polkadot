// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Parachains support in Rialto runtime.

use crate::{
	AccountId, Babe, Balance, Balances, BlockNumber, Call, Event, Origin, Registrar, Runtime,
	Slots, UncheckedExtrinsic,
};

use frame_support::{parameter_types, weights::Weight};
use frame_system::EnsureRoot;
use polkadot_primitives::v2::ValidatorIndex;
use polkadot_runtime_common::{paras_registrar, paras_sudo_wrapper, slots};
use polkadot_runtime_parachains::{
	configuration as parachains_configuration, dmp as parachains_dmp, hrmp as parachains_hrmp,
	inclusion as parachains_inclusion, initializer as parachains_initializer,
	origin as parachains_origin, paras as parachains_paras,
	paras_inherent as parachains_paras_inherent, scheduler as parachains_scheduler,
	session_info as parachains_session_info, shared as parachains_shared, ump as parachains_ump,
};
use sp_runtime::transaction_validity::TransactionPriority;

impl<C> frame_system::offchain::SendTransactionTypes<C> for Runtime
where
	Call: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = Call;
}

/// Special `RewardValidators` that does nothing ;)
pub struct RewardValidators;
impl polkadot_runtime_parachains::inclusion::RewardValidators for RewardValidators {
	fn reward_backing(_: impl IntoIterator<Item = ValidatorIndex>) {}
	fn reward_bitfields(_: impl IntoIterator<Item = ValidatorIndex>) {}
}

// all required parachain modules from `polkadot-runtime-parachains` crate

impl parachains_configuration::Config for Runtime {
	type WeightInfo = parachains_configuration::TestWeightInfo;
}

impl parachains_dmp::Config for Runtime {}

impl parachains_hrmp::Config for Runtime {
	type Event = Event;
	type Origin = Origin;
	type Currency = Balances;
	type WeightInfo = parachains_hrmp::TestWeightInfo;
}

impl parachains_inclusion::Config for Runtime {
	type Event = Event;
	type RewardValidators = RewardValidators;
	type DisputesHandler = ();
}

impl parachains_initializer::Config for Runtime {
	type Randomness = pallet_babe::RandomnessFromOneEpochAgo<Runtime>;
	type ForceOrigin = EnsureRoot<AccountId>;
	type WeightInfo = ();
}

impl parachains_origin::Config for Runtime {}

parameter_types! {
	pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl parachains_paras::Config for Runtime {
	type Event = Event;
	type WeightInfo = parachains_paras::TestWeightInfo;
	type UnsignedPriority = ParasUnsignedPriority;
	type NextSessionRotation = Babe;
}

impl parachains_paras_inherent::Config for Runtime {
	type WeightInfo = parachains_paras_inherent::TestWeightInfo;
}

impl parachains_scheduler::Config for Runtime {}

impl parachains_session_info::Config for Runtime {}

impl parachains_shared::Config for Runtime {}

parameter_types! {
	pub const FirstMessageFactorPercent: u64 = 100;
}

impl parachains_ump::Config for Runtime {
	type Event = Event;
	type UmpSink = ();
	type FirstMessageFactorPercent = FirstMessageFactorPercent;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
	type WeightInfo = parachains_ump::TestWeightInfo;
}

// required onboarding pallets. We're not going to use auctions or crowdloans, so they're missing

parameter_types! {
	pub const ParaDeposit: Balance = 0;
	pub const DataDepositPerByte: Balance = 0;
}

impl paras_registrar::Config for Runtime {
	type Event = Event;
	type Origin = Origin;
	type Currency = Balances;
	type OnSwap = Slots;
	type ParaDeposit = ParaDeposit;
	type DataDepositPerByte = DataDepositPerByte;
	type WeightInfo = paras_registrar::TestWeightInfo;
}

parameter_types! {
	pub const LeasePeriod: BlockNumber = 10 * bp_rialto::MINUTES;
}

impl slots::Config for Runtime {
	type Event = Event;
	type Currency = Balances;
	type Registrar = Registrar;
	type LeasePeriod = LeasePeriod;
	type WeightInfo = slots::TestWeightInfo;
	type LeaseOffset = ();
	type ForceOrigin = EnsureRoot<AccountId>;
}

impl paras_sudo_wrapper::Config for Runtime {}

pub struct ZeroWeights;

impl polkadot_runtime_common::paras_registrar::WeightInfo for ZeroWeights {
	fn reserve() -> Weight {
		0
	}
	fn register() -> Weight {
		0
	}
	fn force_register() -> Weight {
		0
	}
	fn deregister() -> Weight {
		0
	}
	fn swap() -> Weight {
		0
	}
}

impl polkadot_runtime_common::slots::WeightInfo for ZeroWeights {
	fn force_lease() -> Weight {
		0
	}
	fn manage_lease_period_start(_c: u32, _t: u32) -> Weight {
		0
	}
	fn clear_all_leases() -> Weight {
		0
	}
	fn trigger_onboard() -> Weight {
		0
	}
}
