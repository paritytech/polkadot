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
	new_test_ext, Configuration, Event as MockEvent, Hrmp, MockGenesisConfig, Paras, ParasShared,
	System, Test,
};
use frame_support::{assert_noop, assert_ok, traits::Currency as _};
use primitives::v2::BlockNumber;
use std::collections::BTreeMap;

fn run_to_block(to: BlockNumber, new_session: Option<Vec<BlockNumber>>) {
	let config = Configuration::config();
	while System::block_number() < to {
		let b = System::block_number();

		// NOTE: this is in reverse initialization order.
		Hrmp::initializer_finalize();
		Paras::initializer_finalize(b);
		ParasShared::initializer_finalize();

		if new_session.as_ref().map_or(false, |v| v.contains(&(b + 1))) {
			let notification = crate::initializer::SessionChangeNotification {
				prev_config: config.clone(),
				new_config: config.clone(),
				session_index: ParasShared::session_index() + 1,
				..Default::default()
			};

			// NOTE: this is in initialization order.
			ParasShared::initializer_on_new_session(
				notification.session_index,
				notification.random_seed,
				&notification.new_config,
				notification.validators.clone(),
			);
			let outgoing_paras = Paras::initializer_on_new_session(&notification);
			Hrmp::initializer_on_new_session(&notification, &outgoing_paras);
		}

		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		// NOTE: this is in initialization order.
		ParasShared::initializer_initialize(b + 1);
		Paras::initializer_initialize(b + 1);
		Hrmp::initializer_initialize(b + 1);
	}
}

#[derive(Debug)]
pub(super) struct GenesisConfigBuilder {
	hrmp_channel_max_capacity: u32,
	hrmp_channel_max_message_size: u32,
	hrmp_max_parathread_outbound_channels: u32,
	hrmp_max_parachain_outbound_channels: u32,
	hrmp_max_parathread_inbound_channels: u32,
	hrmp_max_parachain_inbound_channels: u32,
	hrmp_max_message_num_per_candidate: u32,
	hrmp_channel_max_total_size: u32,
	hrmp_sender_deposit: Balance,
	hrmp_recipient_deposit: Balance,
}

impl Default for GenesisConfigBuilder {
	fn default() -> Self {
		Self {
			hrmp_channel_max_capacity: 2,
			hrmp_channel_max_message_size: 8,
			hrmp_max_parathread_outbound_channels: 1,
			hrmp_max_parachain_outbound_channels: 2,
			hrmp_max_parathread_inbound_channels: 1,
			hrmp_max_parachain_inbound_channels: 2,
			hrmp_max_message_num_per_candidate: 2,
			hrmp_channel_max_total_size: 16,
			hrmp_sender_deposit: 100,
			hrmp_recipient_deposit: 100,
		}
	}
}

impl GenesisConfigBuilder {
	pub(super) fn build(self) -> crate::mock::MockGenesisConfig {
		let mut genesis = default_genesis_config();
		let config = &mut genesis.configuration.config;
		config.hrmp_channel_max_capacity = self.hrmp_channel_max_capacity;
		config.hrmp_channel_max_message_size = self.hrmp_channel_max_message_size;
		config.hrmp_max_parathread_outbound_channels = self.hrmp_max_parathread_outbound_channels;
		config.hrmp_max_parachain_outbound_channels = self.hrmp_max_parachain_outbound_channels;
		config.hrmp_max_parathread_inbound_channels = self.hrmp_max_parathread_inbound_channels;
		config.hrmp_max_parachain_inbound_channels = self.hrmp_max_parachain_inbound_channels;
		config.hrmp_max_message_num_per_candidate = self.hrmp_max_message_num_per_candidate;
		config.hrmp_channel_max_total_size = self.hrmp_channel_max_total_size;
		config.hrmp_sender_deposit = self.hrmp_sender_deposit;
		config.hrmp_recipient_deposit = self.hrmp_recipient_deposit;
		genesis
	}
}

fn default_genesis_config() -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: crate::configuration::HostConfiguration {
				max_downward_message_size: 1024,
				pvf_checking_enabled: false,
				..Default::default()
			},
		},
		..Default::default()
	}
}

fn register_parachain_with_balance(id: ParaId, balance: Balance) {
	assert_ok!(Paras::schedule_para_initialize(
		id,
		crate::paras::ParaGenesisArgs {
			parachain: true,
			genesis_head: vec![1].into(),
			validation_code: vec![1].into(),
		},
	));
	<Test as Config>::Currency::make_free_balance_be(&id.into_account(), balance);
}

fn register_parachain(id: ParaId) {
	register_parachain_with_balance(id, 1000);
}

fn deregister_parachain(id: ParaId) {
	assert_ok!(Paras::schedule_para_cleanup(id));
}

fn channel_exists(sender: ParaId, recipient: ParaId) -> bool {
	<Hrmp as Store>::HrmpChannels::get(&HrmpChannelId { sender, recipient }).is_some()
}

#[test]
fn empty_state_consistent_state() {
	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		Hrmp::assert_storage_consistency_exhaustive();
	});
}

#[test]
fn open_channel_works() {
	let para_a = 1.into();
	let para_a_origin: crate::Origin = 1.into();
	let para_b = 3.into();
	let para_b_origin: crate::Origin = 3.into();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		// We need both A & B to be registered and alive parachains.
		register_parachain(para_a);
		register_parachain(para_b);

		run_to_block(5, Some(vec![4, 5]));
		Hrmp::hrmp_init_open_channel(para_a_origin.into(), para_b, 2, 8).unwrap();
		Hrmp::assert_storage_consistency_exhaustive();
		assert!(System::events().iter().any(|record| record.event ==
			MockEvent::Hrmp(Event::OpenChannelRequested(para_a, para_b, 2, 8))));

		Hrmp::hrmp_accept_open_channel(para_b_origin.into(), para_a).unwrap();
		Hrmp::assert_storage_consistency_exhaustive();
		assert!(System::events()
			.iter()
			.any(|record| record.event ==
				MockEvent::Hrmp(Event::OpenChannelAccepted(para_a, para_b))));

		// Advance to a block 6, but without session change. That means that the channel has
		// not been created yet.
		run_to_block(6, None);
		assert!(!channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();

		// Now let the session change happen and thus open the channel.
		run_to_block(8, Some(vec![8]));
		assert!(channel_exists(para_a, para_b));
	});
}

#[test]
fn close_channel_works() {
	let para_a = 5.into();
	let para_b = 2.into();
	let para_b_origin: crate::Origin = 2.into();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain(para_a);
		register_parachain(para_b);

		run_to_block(5, Some(vec![4, 5]));
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();

		run_to_block(6, Some(vec![6]));
		assert!(channel_exists(para_a, para_b));

		// Close the channel. The effect is not immediate, but rather deferred to the next
		// session change.
		let channel_id = HrmpChannelId { sender: para_a, recipient: para_b };
		Hrmp::hrmp_close_channel(para_b_origin.into(), channel_id.clone()).unwrap();
		assert!(channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();

		// After the session change the channel should be closed.
		run_to_block(8, Some(vec![8]));
		assert!(!channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();
		assert!(System::events().iter().any(|record| record.event ==
			MockEvent::Hrmp(Event::ChannelClosed(para_b, channel_id.clone()))));
	});
}

#[test]
fn send_recv_messages() {
	let para_a = 32.into();
	let para_b = 64.into();

	let mut genesis = GenesisConfigBuilder::default();
	genesis.hrmp_channel_max_message_size = 20;
	genesis.hrmp_channel_max_total_size = 20;
	new_test_ext(genesis.build()).execute_with(|| {
		register_parachain(para_a);
		register_parachain(para_b);

		run_to_block(5, Some(vec![4, 5]));
		Hrmp::init_open_channel(para_a, para_b, 2, 20).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();

		// On Block 6:
		// A sends a message to B
		run_to_block(6, Some(vec![6]));
		assert!(channel_exists(para_a, para_b));
		let msgs =
			vec![OutboundHrmpMessage { recipient: para_b, data: b"this is an emergency".to_vec() }];
		let config = Configuration::config();
		assert!(Hrmp::check_outbound_hrmp(&config, para_a, &msgs).is_ok());
		let _ = Hrmp::queue_outbound_hrmp(para_a, msgs);
		Hrmp::assert_storage_consistency_exhaustive();

		// On Block 7:
		// B receives the message sent by A. B sets the watermark to 6.
		run_to_block(7, None);
		assert!(Hrmp::check_hrmp_watermark(para_b, 7, 6).is_ok());
		let _ = Hrmp::prune_hrmp(para_b, 6);
		Hrmp::assert_storage_consistency_exhaustive();
	});
}

#[test]
fn hrmp_mqc_head_fixture() {
	let para_a = 2000.into();
	let para_b = 2024.into();

	let mut genesis = GenesisConfigBuilder::default();
	genesis.hrmp_channel_max_message_size = 20;
	genesis.hrmp_channel_max_total_size = 20;
	new_test_ext(genesis.build()).execute_with(|| {
		register_parachain(para_a);
		register_parachain(para_b);

		run_to_block(2, Some(vec![1, 2]));
		Hrmp::init_open_channel(para_a, para_b, 2, 20).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();

		run_to_block(3, Some(vec![3]));
		let _ = Hrmp::queue_outbound_hrmp(
			para_a,
			vec![OutboundHrmpMessage { recipient: para_b, data: vec![1, 2, 3] }],
		);

		run_to_block(4, None);
		let _ = Hrmp::queue_outbound_hrmp(
			para_a,
			vec![OutboundHrmpMessage { recipient: para_b, data: vec![4, 5, 6] }],
		);

		assert_eq!(
			Hrmp::hrmp_mqc_heads(para_b),
			vec![(
				para_a,
				hex_literal::hex![
					"a964fd3b4f3d3ce92a0e25e576b87590d92bb5cb7031909c7f29050e1f04a375"
				]
				.into()
			),],
		);
	});
}

#[test]
fn accept_incoming_request_and_offboard() {
	let para_a = 32.into();
	let para_b = 64.into();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain(para_a);
		register_parachain(para_b);

		run_to_block(5, Some(vec![4, 5]));
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();
		deregister_parachain(para_a);

		// On Block 7: 2x session change. The channel should not be created.
		run_to_block(7, Some(vec![6, 7]));
		assert!(!Paras::is_valid_para(para_a));
		assert!(!channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();
	});
}

#[test]
fn check_sent_messages() {
	let para_a = 32.into();
	let para_b = 64.into();
	let para_c = 97.into();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain(para_a);
		register_parachain(para_b);
		register_parachain(para_c);

		run_to_block(5, Some(vec![4, 5]));

		// Open two channels to the same receiver, b:
		// a -> b, c -> b
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();
		Hrmp::init_open_channel(para_c, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_c).unwrap();

		// On Block 6: session change.
		run_to_block(6, Some(vec![6]));
		assert!(Paras::is_valid_para(para_a));

		let msgs = vec![OutboundHrmpMessage { recipient: para_b, data: b"knock".to_vec() }];
		let config = Configuration::config();
		assert!(Hrmp::check_outbound_hrmp(&config, para_a, &msgs).is_ok());
		let _ = Hrmp::queue_outbound_hrmp(para_a, msgs.clone());

		// Verify that the sent messages are there and that also the empty channels are present.
		let mqc_heads = Hrmp::hrmp_mqc_heads(para_b);
		let contents = Hrmp::inbound_hrmp_channels_contents(para_b);
		assert_eq!(
			contents,
			vec![
				(para_a, vec![InboundHrmpMessage { sent_at: 6, data: b"knock".to_vec() }]),
				(para_c, vec![])
			]
			.into_iter()
			.collect::<BTreeMap::<_, _>>(),
		);
		assert_eq!(
			mqc_heads,
			vec![
				(
					para_a,
					hex_literal::hex!(
						"3bba6404e59c91f51deb2ae78f1273ebe75896850713e13f8c0eba4b0996c483"
					)
					.into()
				),
				(para_c, Default::default())
			],
		);

		Hrmp::assert_storage_consistency_exhaustive();
	});
}

#[test]
fn verify_externally_accessible() {
	use primitives::v2::{well_known_keys, AbridgedHrmpChannel};

	let para_a = 20.into();
	let para_b = 21.into();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		// Register two parachains, wait until a session change, then initiate channel open
		// request and accept that, and finally wait until the next session.
		register_parachain(para_a);
		register_parachain(para_b);
		run_to_block(5, Some(vec![4, 5]));
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();
		run_to_block(8, Some(vec![8]));

		// Here we have a channel a->b opened.
		//
		// Try to obtain this channel from the storage and
		// decode it into the abridged version.
		assert!(channel_exists(para_a, para_b));
		let raw_hrmp_channel =
			sp_io::storage::get(&well_known_keys::hrmp_channels(HrmpChannelId {
				sender: para_a,
				recipient: para_b,
			}))
			.expect("the channel exists and we must be able to get it through well known keys");
		let abridged_hrmp_channel = AbridgedHrmpChannel::decode(&mut &raw_hrmp_channel[..])
			.expect("HrmpChannel should be decodable as AbridgedHrmpChannel");

		assert_eq!(
			abridged_hrmp_channel,
			AbridgedHrmpChannel {
				max_capacity: 2,
				max_total_size: 16,
				max_message_size: 8,
				msg_count: 0,
				total_size: 0,
				mqc_head: None,
			},
		);

		let raw_ingress_index =
			sp_io::storage::get(&well_known_keys::hrmp_ingress_channel_index(para_b))
				.expect("the ingress index must be present for para_b");
		let ingress_index = <Vec<ParaId>>::decode(&mut &raw_ingress_index[..])
			.expect("ingress indexx should be decodable as a list of para ids");
		assert_eq!(ingress_index, vec![para_a]);

		// Now, verify that we can access and decode the egress index.
		let raw_egress_index =
			sp_io::storage::get(&well_known_keys::hrmp_egress_channel_index(para_a))
				.expect("the egress index must be present for para_a");
		let egress_index = <Vec<ParaId>>::decode(&mut &raw_egress_index[..])
			.expect("egress index should be decodable as a list of para ids");
		assert_eq!(egress_index, vec![para_b]);
	});
}

#[test]
fn charging_deposits() {
	let para_a = 32.into();
	let para_b = 64.into();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain_with_balance(para_a, 0);
		register_parachain(para_b);
		run_to_block(5, Some(vec![4, 5]));

		assert_noop!(
			Hrmp::init_open_channel(para_a, para_b, 2, 8),
			pallet_balances::Error::<Test, _>::InsufficientBalance
		);
	});

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain(para_a);
		register_parachain_with_balance(para_b, 0);
		run_to_block(5, Some(vec![4, 5]));

		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();

		assert_noop!(
			Hrmp::accept_open_channel(para_b, para_a),
			pallet_balances::Error::<Test, _>::InsufficientBalance
		);
	});
}

#[test]
fn refund_deposit_on_normal_closure() {
	let para_a = 32.into();
	let para_b = 64.into();

	let mut genesis = GenesisConfigBuilder::default();
	genesis.hrmp_sender_deposit = 20;
	genesis.hrmp_recipient_deposit = 15;
	new_test_ext(genesis.build()).execute_with(|| {
		// Register two parachains funded with different amounts of funds and arrange a channel.
		register_parachain_with_balance(para_a, 100);
		register_parachain_with_balance(para_b, 110);
		run_to_block(5, Some(vec![4, 5]));
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();
		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 80);
		assert_eq!(<Test as Config>::Currency::free_balance(&para_b.into_account()), 95);
		run_to_block(8, Some(vec![8]));

		// Now, we close the channel and wait until the next session.
		Hrmp::close_channel(para_b, HrmpChannelId { sender: para_a, recipient: para_b }).unwrap();
		run_to_block(10, Some(vec![10]));
		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 100);
		assert_eq!(<Test as Config>::Currency::free_balance(&para_b.into_account()), 110);
	});
}

#[test]
fn refund_deposit_on_offboarding() {
	let para_a = 32.into();
	let para_b = 64.into();

	let mut genesis = GenesisConfigBuilder::default();
	genesis.hrmp_sender_deposit = 20;
	genesis.hrmp_recipient_deposit = 15;
	new_test_ext(genesis.build()).execute_with(|| {
		// Register two parachains and open a channel between them.
		register_parachain_with_balance(para_a, 100);
		register_parachain_with_balance(para_b, 110);
		run_to_block(5, Some(vec![4, 5]));
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		Hrmp::accept_open_channel(para_b, para_a).unwrap();
		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 80);
		assert_eq!(<Test as Config>::Currency::free_balance(&para_b.into_account()), 95);
		run_to_block(8, Some(vec![8]));
		assert!(channel_exists(para_a, para_b));

		// Then deregister one parachain.
		deregister_parachain(para_a);
		run_to_block(10, Some(vec![9, 10]));

		// The channel should be removed.
		assert!(!Paras::is_valid_para(para_a));
		assert!(!channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();

		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 100);
		assert_eq!(<Test as Config>::Currency::free_balance(&para_b.into_account()), 110);
	});
}

#[test]
fn no_dangling_open_requests() {
	let para_a = 32.into();
	let para_b = 64.into();

	let mut genesis = GenesisConfigBuilder::default();
	genesis.hrmp_sender_deposit = 20;
	genesis.hrmp_recipient_deposit = 15;
	new_test_ext(genesis.build()).execute_with(|| {
		// Register two parachains and open a channel between them.
		register_parachain_with_balance(para_a, 100);
		register_parachain_with_balance(para_b, 110);
		run_to_block(5, Some(vec![4, 5]));

		// Start opening a channel a->b
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 80);

		// Then deregister one parachain, but don't wait two sessions until it takes effect.
		// Instead, `para_b` will confirm the request, which will take place the same time
		// the offboarding should happen.
		deregister_parachain(para_a);
		run_to_block(9, Some(vec![9]));
		Hrmp::accept_open_channel(para_b, para_a).unwrap();
		assert_eq!(<Test as Config>::Currency::free_balance(&para_b.into_account()), 95);
		assert!(!channel_exists(para_a, para_b));
		run_to_block(10, Some(vec![10]));

		// The outcome we expect is `para_b` should receive the refund.
		assert_eq!(<Test as Config>::Currency::free_balance(&para_b.into_account()), 110);
		assert!(!channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();
	});
}

#[test]
fn cancel_pending_open_channel_request() {
	let para_a = 32.into();
	let para_b = 64.into();

	let mut genesis = GenesisConfigBuilder::default();
	genesis.hrmp_sender_deposit = 20;
	genesis.hrmp_recipient_deposit = 15;
	new_test_ext(genesis.build()).execute_with(|| {
		// Register two parachains and open a channel between them.
		register_parachain_with_balance(para_a, 100);
		register_parachain_with_balance(para_b, 110);
		run_to_block(5, Some(vec![4, 5]));

		// Start opening a channel a->b
		Hrmp::init_open_channel(para_a, para_b, 2, 8).unwrap();
		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 80);

		// Cancel opening the channel
		Hrmp::cancel_open_request(para_a, HrmpChannelId { sender: para_a, recipient: para_b })
			.unwrap();
		assert_eq!(<Test as Config>::Currency::free_balance(&para_a.into_account()), 100);

		run_to_block(10, Some(vec![10]));
		assert!(!channel_exists(para_a, para_b));
		Hrmp::assert_storage_consistency_exhaustive();
	});
}
