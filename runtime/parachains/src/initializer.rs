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

//! This module is responsible for maintaining a consistent initialization order for all other
//! parachains modules. It's also responsible for finalization and session change notifications.
//!
//! This module can throw fatal errors if session-change notifications are received after initialization.

use sp_std::prelude::*;
use frame_support::weights::Weight;
use primitives::{
	parachain::{ValidatorId},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
};

pub trait Trait: system::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as Initializer {
		/// Whether the parachains modules have been initialized within this block.
		/// Should never hit the trie.
		HasInitialized: Option<()>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The initializer module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;

		fn on_initialize(_now: T::BlockNumber) -> Weight {
			HasInitialized::set(Some(()));

			0
		}

		fn on_finalize() {
			HasInitialized::take();
		}
	}
}

impl<T: Trait> Module<T> {
	/// Should be called when a new session occurs. Forwards the session notification to all
	/// wrapped modules.
	///
	/// Panics if the modules have already been initialized.
	fn on_new_session<'a, I: 'a>(_changed: bool, _validators: I, _queued: I)
		where I: Iterator<Item=(&'a T::AccountId, ValidatorId)>
	{
		assert!(HasInitialized::get().is_none());
	}
}

impl<T: Trait> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = ValidatorId;
}

impl<T: Trait> session::OneSessionHandler<T::AccountId> for Module<T> {
	type Key = ValidatorId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{

	}

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		<Module<T>>::on_new_session(changed, validators, queued);
	}

	fn on_disabled(_i: usize) { }
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_io::TestExternalities;
	use sp_core::{H256};
	use sp_runtime::{
		Perbill,
		traits::{
			BlakeTwo256, IdentityLookup,
		},
	};
	use primitives::{
		BlockNumber,
		Header,
	};
	use frame_support::{
		impl_outer_origin, impl_outer_dispatch, parameter_types,
		traits::{OnInitialize, OnFinalize},
	};

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;

	impl_outer_origin! {
		pub enum Origin for Test { }
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			initializer::Initializer,
		}
	}

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub const MaximumBlockWeight: Weight = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}

	impl system::Trait for Test {
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
		type OnKilledAccount = ();
	}

	impl Trait for Test { }

	type Initializer = Module<Test>;

	fn new_test_ext() -> TestExternalities {
		let t = system::GenesisConfig::default().build_storage::<Test>().unwrap();

		t.into()
	}

	#[test]
	#[should_panic]
	fn panics_if_session_changes_after_on_initialize() {
		new_test_ext().execute_with(|| {
			Initializer::on_initialize(1);
			Initializer::on_new_session(false, Vec::new().into_iter(), Vec::new().into_iter());
		});
	}

	#[test]
	fn sets_flag_on_initialize() {
		new_test_ext().execute_with(|| {
			Initializer::on_initialize(1);

			assert!(HasInitialized::get().is_some());
		})
	}

	#[test]
	fn clears_flag_on_finalize() {
		new_test_ext().execute_with(|| {
			Initializer::on_initialize(1);
			Initializer::on_finalize(1);

			assert!(HasInitialized::get().is_none());
		})
	}
}
