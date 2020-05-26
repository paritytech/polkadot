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

//! Configuration manager for the Polkadot runtime parachains logic.
//!
//! Configuration can change only at session boundaries and is buffered until then.

use sp_std::prelude::*;
use frame_support::weights::Weight;
use primitives::{
	parachain::{ValidatorId},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
};
use codec::{Encode, Decode};


/// All configuration of the runtime with respect to parachains and parathreads.
#[derive(Clone, Encode, Decode, PartialEq, Default)]
pub struct HostConfiguration<BlockNumber: Default> {
	/// The minimum frequency at which parachains can update their validation code.
	pub validation_upgrade_frequency: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,
	/// The acceptance period, in blocks. This is the amount of blocks after availability that validators
	/// and fishermen have to perform secondary checks or issue reports.
	pub acceptance_period: BlockNumber,
	/// The maximum validation code size, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size, in bytes.
	pub max_head_data_size: u32,
	/// The amount of execution cores to dedicate to parathread execution.
	pub parathread_cores: u32,
	/// The number of retries that a parathread author has to submit their block.
	pub parathread_retries: u32,
	/// How often parachain groups should be rotated across parachains.
	pub parachain_rotation_frequency: BlockNumber,
	/// The availability period, in blocks, for parachains. This is the amount of blocks
	/// after inclusion that validators have to make the block available and signal its availability to
	/// the chain. Must be at least 1.
	pub chain_availability_period: BlockNumber,
	/// The availability period, in blocks, for parathreads. Same as the `chain_availability_period`,
	/// but a differing timeout due to differing requirements. Must be at least 1.
	pub thread_availability_period: BlockNumber,
	/// The amount of blocks ahead to schedule parachains and parathreads.
	pub scheduling_lookahead: u32,
}

pub trait Trait: system::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as Configuration {
		/// The active configuration for the current session.
		Config get(fn config) config(): HostConfiguration<T::BlockNumber>;
		/// Pending configuration (if any) for the next session.
		PendingConfig: Option<HostConfiguration<T::BlockNumber>>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The parachains configuration module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {
	/// Called by the initializer to initialize the configuration module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the configuration module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(_validators: &[ValidatorId], _queued: &[ValidatorId]) {
		if let Some(pending) = <Self as Store>::PendingConfig::take() {
			<Self as Store>::Config::set(pending);
		}
	}
}
