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
use crate::mock::{new_test_ext, Configuration, Dmp, MockGenesisConfig, Paras, System};
use hex_literal::hex;
use parity_scale_codec::Encode;
use primitives::v1::BlockNumber;

pub(crate) fn run_to_block(to: BlockNumber, new_session: Option<Vec<BlockNumber>>) {
	while System::block_number() < to {
		let b = System::block_number();
		Paras::initializer_finalize(b);
		Dmp::initializer_finalize();
		if new_session.as_ref().map_or(false, |v| v.contains(&(b + 1))) {
			Dmp::initializer_on_new_session(&Default::default(), &Vec::new());
		}
		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		Paras::initializer_finalize(b + 1);
		Dmp::initializer_initialize(b + 1);
	}
}

fn default_genesis_config() -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: crate::configuration::HostConfiguration {
				max_downward_message_size: 1024,
				..Default::default()
			},
		},
		..Default::default()
	}
}

fn queue_downward_message(
	para_id: ParaId,
	msg: DownwardMessage,
) -> Result<(), QueueDownwardMessageError> {
	Dmp::queue_downward_message(&Configuration::config(), para_id, msg)
}

#[test]
fn clean_dmp_works() {
	let a = ParaId::from(1312);
	let b = ParaId::from(228);
	let c = ParaId::from(123);

	new_test_ext(default_genesis_config()).execute_with(|| {
		// enqueue downward messages to A, B and C.
		queue_downward_message(a, vec![1, 2, 3]).unwrap();
		queue_downward_message(b, vec![4, 5, 6]).unwrap();
		queue_downward_message(c, vec![7, 8, 9]).unwrap();

		let notification = crate::initializer::SessionChangeNotification::default();
		let outgoing_paras = vec![a, b];
		Dmp::initializer_on_new_session(&notification, &outgoing_paras);

		assert!(<Dmp as Store>::DownwardMessageQueues::get(&a).is_empty());
		assert!(<Dmp as Store>::DownwardMessageQueues::get(&b).is_empty());
		assert!(!<Dmp as Store>::DownwardMessageQueues::get(&c).is_empty());
	});
}

#[test]
fn dmq_length_and_head_updated_properly() {
	let a = ParaId::from(1312);
	let b = ParaId::from(228);

	new_test_ext(default_genesis_config()).execute_with(|| {
		assert_eq!(Dmp::dmq_length(a), 0);
		assert_eq!(Dmp::dmq_length(b), 0);

		queue_downward_message(a, vec![1, 2, 3]).unwrap();

		assert_eq!(Dmp::dmq_length(a), 1);
		assert_eq!(Dmp::dmq_length(b), 0);
		assert!(!Dmp::dmq_mqc_head(a).is_zero());
		assert!(Dmp::dmq_mqc_head(b).is_zero());
	});
}

#[test]
fn dmp_mqc_head_fixture() {
	let a = ParaId::from(2000);

	new_test_ext(default_genesis_config()).execute_with(|| {
		run_to_block(2, None);
		assert!(Dmp::dmq_mqc_head(a).is_zero());
		queue_downward_message(a, vec![1, 2, 3]).unwrap();

		run_to_block(3, None);
		queue_downward_message(a, vec![4, 5, 6]).unwrap();

		assert_eq!(
			Dmp::dmq_mqc_head(a),
			hex!["88dc00db8cc9d22aa62b87807705831f164387dfa49f80a8600ed1cbe1704b6b"].into(),
		);
	});
}

#[test]
fn check_processed_downward_messages() {
	let a = ParaId::from(1312);

	new_test_ext(default_genesis_config()).execute_with(|| {
		// processed_downward_messages=0 is allowed when the DMQ is empty.
		assert!(Dmp::check_processed_downward_messages(a, 0).is_ok());

		queue_downward_message(a, vec![1, 2, 3]).unwrap();
		queue_downward_message(a, vec![4, 5, 6]).unwrap();
		queue_downward_message(a, vec![7, 8, 9]).unwrap();

		// 0 doesn't pass if the DMQ has msgs.
		assert!(!Dmp::check_processed_downward_messages(a, 0).is_ok());
		// a candidate can consume up to 3 messages
		assert!(Dmp::check_processed_downward_messages(a, 1).is_ok());
		assert!(Dmp::check_processed_downward_messages(a, 2).is_ok());
		assert!(Dmp::check_processed_downward_messages(a, 3).is_ok());
		// there is no 4 messages in the queue
		assert!(!Dmp::check_processed_downward_messages(a, 4).is_ok());
	});
}

#[test]
fn dmq_pruning() {
	let a = ParaId::from(1312);

	new_test_ext(default_genesis_config()).execute_with(|| {
		assert_eq!(Dmp::dmq_length(a), 0);

		queue_downward_message(a, vec![1, 2, 3]).unwrap();
		queue_downward_message(a, vec![4, 5, 6]).unwrap();
		queue_downward_message(a, vec![7, 8, 9]).unwrap();
		assert_eq!(Dmp::dmq_length(a), 3);

		// pruning 0 elements shouldn't change anything.
		Dmp::prune_dmq(a, 0);
		assert_eq!(Dmp::dmq_length(a), 3);

		Dmp::prune_dmq(a, 2);
		assert_eq!(Dmp::dmq_length(a), 1);
	});
}

#[test]
fn queue_downward_message_critical() {
	let a = ParaId::from(1312);

	let mut genesis = default_genesis_config();
	genesis.configuration.config.max_downward_message_size = 7;

	new_test_ext(genesis).execute_with(|| {
		let smol = [0; 3].to_vec();
		let big = [0; 8].to_vec();

		// still within limits
		assert_eq!(smol.encode().len(), 4);
		assert!(queue_downward_message(a, smol).is_ok());

		// that's too big
		assert_eq!(big.encode().len(), 9);
		assert!(queue_downward_message(a, big).is_err());
	});
}

#[test]
fn verify_dmq_mqc_head_is_externally_accessible() {
	use hex_literal::hex;
	use primitives::v1::well_known_keys;

	let a = ParaId::from(2020);

	new_test_ext(default_genesis_config()).execute_with(|| {
		let head = sp_io::storage::get(&well_known_keys::dmq_mqc_head(a));
		assert_eq!(head, None);

		queue_downward_message(a, vec![1, 2, 3]).unwrap();

		let head = sp_io::storage::get(&well_known_keys::dmq_mqc_head(a));
		assert_eq!(
			head,
			Some(hex!["434f8579a2297dfea851bf6be33093c83a78b655a53ae141a7894494c0010589"].to_vec())
		);
	});
}
