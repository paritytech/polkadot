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

use super::*;
use crate::mock::{
	new_test_ext, Configuration, Dmp, Initializer, MockGenesisConfig, Paras, SessionInfo, System,
};
use primitives::v2::{HeadData, Id as ParaId};
use test_helpers::dummy_validation_code;

use frame_support::{
	assert_ok,
	traits::{OnFinalize, OnInitialize},
};

#[test]
fn session_0_is_instantly_applied() {
	new_test_ext(Default::default()).execute_with(|| {
		Initializer::on_new_session(false, 0, Vec::new().into_iter(), Some(Vec::new().into_iter()));

		let v = <Initializer as Store>::BufferedSessionChanges::get();
		assert!(v.is_empty());

		assert_eq!(SessionInfo::earliest_stored_session(), 0);
		assert!(SessionInfo::session_info(0).is_some());
	});
}

#[test]
fn session_change_before_initialize_is_still_buffered_after() {
	new_test_ext(Default::default()).execute_with(|| {
		Initializer::on_new_session(false, 1, Vec::new().into_iter(), Some(Vec::new().into_iter()));

		let now = System::block_number();
		Initializer::on_initialize(now);

		let v = <Initializer as Store>::BufferedSessionChanges::get();
		assert_eq!(v.len(), 1);
	});
}

#[test]
fn session_change_applied_on_finalize() {
	new_test_ext(Default::default()).execute_with(|| {
		Initializer::on_initialize(1);
		Initializer::on_new_session(false, 1, Vec::new().into_iter(), Some(Vec::new().into_iter()));

		Initializer::on_finalize(1);

		assert!(<Initializer as Store>::BufferedSessionChanges::get().is_empty());
	});
}

#[test]
fn sets_flag_on_initialize() {
	new_test_ext(Default::default()).execute_with(|| {
		Initializer::on_initialize(1);

		assert!(<Initializer as Store>::HasInitialized::get().is_some());
	})
}

#[test]
fn clears_flag_on_finalize() {
	new_test_ext(Default::default()).execute_with(|| {
		Initializer::on_initialize(1);
		Initializer::on_finalize(1);

		assert!(<Initializer as Store>::HasInitialized::get().is_none());
	})
}

#[test]
fn scheduled_cleanup_performed() {
	let a = ParaId::from(1312);
	let b = ParaId::from(228);
	let c = ParaId::from(123);

	let mock_genesis = crate::paras::ParaGenesisArgs {
		parachain: true,
		genesis_head: HeadData(vec![4, 5, 6]),
		validation_code: dummy_validation_code(),
	};

	new_test_ext(MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: crate::configuration::HostConfiguration {
				max_downward_message_size: 1024,
				..Default::default()
			},
		},
		paras: crate::paras::GenesisConfig {
			paras: vec![
				(a, mock_genesis.clone()),
				(b, mock_genesis.clone()),
				(c, mock_genesis.clone()),
			],
			..Default::default()
		},
		..Default::default()
	})
	.execute_with(|| {
		// enqueue downward messages to A, B and C.
		assert_ok!(Dmp::queue_downward_message(&Configuration::config(), a, vec![1, 2, 3]));
		assert_ok!(Dmp::queue_downward_message(&Configuration::config(), b, vec![4, 5, 6]));
		assert_ok!(Dmp::queue_downward_message(&Configuration::config(), c, vec![7, 8, 9]));

		assert_ok!(Paras::schedule_para_cleanup(a));
		assert_ok!(Paras::schedule_para_cleanup(b));

		// Apply session 2 in the future
		Initializer::apply_new_session(2, vec![], vec![]);

		assert!(Dmp::dmq_contents(a).is_empty());
		assert!(Dmp::dmq_contents(b).is_empty());
		assert!(!Dmp::dmq_contents(c).is_empty());
	});
}
