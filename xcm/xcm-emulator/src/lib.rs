// Copyright 2023 Parity Technologies (UK) Ltd.
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

pub use codec::Encode;
pub use paste;

pub use frame_support::{
	traits::{Get, Hooks},
	weights::Weight,
};
pub use frame_system;
pub use sp_arithmetic::traits::Bounded;
pub use sp_io::TestExternalities;
pub use sp_std::{cell::RefCell, collections::vec_deque::VecDeque, marker::PhantomData};

pub use cumulus_pallet_dmp_queue;
pub use cumulus_pallet_parachain_system;
pub use cumulus_pallet_xcmp_queue;
pub use cumulus_primitives_core::{
	self, relay_chain::BlockNumber as RelayBlockNumber, DmpMessageHandler, ParaId,
	PersistedValidationData, XcmpMessageHandler,
};
pub use cumulus_primitives_parachain_inherent::ParachainInherentData;
pub use cumulus_test_relay_sproof_builder::RelayStateSproofBuilder;
pub use parachain_info;

pub use polkadot_primitives;
pub use polkadot_runtime_parachains::{
	dmp,
	ump::{MessageId, UmpSink, XcmSink},
};
pub use xcm::{v3::prelude::*, VersionedXcm};
pub use xcm_executor::XcmExecutor;

pub trait TestExt {
	fn new_ext() -> sp_io::TestExternalities;
	fn reset_ext();
	fn execute_with<R>(execute: impl FnOnce() -> R) -> R;
}

#[macro_export]
macro_rules! decl_test_relay_chain {
	(
		pub struct $name:ident {
			Runtime = $runtime:path,
			XcmConfig = $xcm_config:path,
			new_ext = $new_ext:expr,
		}
	) => {
		pub struct $name;

		$crate::__impl_ext_for_relay_chain!($name, $runtime, $new_ext);

		impl $crate::UmpSink for $name {
			fn process_upward_message(
				origin: $crate::ParaId,
				msg: &[u8],
				max_weight: $crate::Weight,
			) -> Result<$crate::Weight, ($crate::MessageId, $crate::Weight)> {
				use $crate::{TestExt, UmpSink};

				Self::execute_with(|| {
					$crate::XcmSink::<$crate::XcmExecutor<$xcm_config>, $runtime>::process_upward_message(origin, msg, max_weight)
				})
			}
		}
	};
}

#[macro_export]
macro_rules! decl_test_parachain {
	(
		pub struct $name:ident {
			Runtime = $runtime:path,
			RuntimeOrigin = $origin:path,
			XcmpMessageHandler = $xcmp_message_handler:path,
			DmpMessageHandler = $dmp_message_handler:path,
			new_ext = $new_ext:expr,
		}
	) => {
		pub struct $name;

		$crate::__impl_ext_for_parachain!($name, $runtime, $origin, $new_ext);

		impl $crate::XcmpMessageHandler for $name {
			fn handle_xcmp_messages<
				'a,
				I: Iterator<Item = ($crate::ParaId, $crate::RelayBlockNumber, &'a [u8])>,
			>(
				iter: I,
				max_weight: $crate::Weight,
			) -> $crate::Weight {
				use $crate::{TestExt, XcmpMessageHandler};

				$name::execute_with(|| {
					<$xcmp_message_handler>::handle_xcmp_messages(iter, max_weight)
				})
			}
		}

		impl $crate::DmpMessageHandler for $name {
			fn handle_dmp_messages(
				iter: impl Iterator<Item = ($crate::RelayBlockNumber, Vec<u8>)>,
				max_weight: $crate::Weight,
			) -> $crate::Weight {
				use $crate::{DmpMessageHandler, TestExt};

				$name::execute_with(|| {
					<$dmp_message_handler>::handle_dmp_messages(iter, max_weight)
				})
			}
		}
	};
}

#[macro_export]
macro_rules! __impl_ext_for_relay_chain {
	// entry point: generate ext name
	($name:ident, $runtime:path, $new_ext:expr) => {
		$crate::paste::paste! {
			$crate::__impl_ext_for_relay_chain!(@impl $name, $runtime, $new_ext, [<EXT_ $name:upper>]);
		}
	};
	// impl
	(@impl $name:ident, $runtime:path, $new_ext:expr, $ext_name:ident) => {
		thread_local! {
			pub static $ext_name: $crate::RefCell<$crate::TestExternalities>
				= $crate::RefCell::new($new_ext);
		}

		impl $crate::TestExt for $name {
			fn new_ext() -> $crate::TestExternalities {
				$new_ext
			}

			fn reset_ext() {
				$ext_name.with(|v| *v.borrow_mut() = $new_ext);
			}

			fn execute_with<R>(execute: impl FnOnce() -> R) -> R {
				let r = $ext_name.with(|v| v.borrow_mut().execute_with(execute));

				// send messages if needed
				$ext_name.with(|v| {
					v.borrow_mut().execute_with(|| {
						use $crate::polkadot_primitives::runtime_api::runtime_decl_for_parachain_host::ParachainHostV4;

						//TODO: mark sent count & filter out sent msg
						for para_id in _para_ids() {
							// downward messages
							let downward_messages = <$runtime>::dmq_contents(para_id.into())
								.into_iter()
								.map(|inbound| (inbound.sent_at, inbound.msg));
							if downward_messages.len() == 0 {
								continue;
							}
							_Messenger::send_downward_messages(para_id, downward_messages.into_iter());

							// Note: no need to handle horizontal messages, as the
							// simulator directly sends them to dest (not relayed).
						}
					})
				});

				_process_messages();

				r
			}
		}
	};
}

#[macro_export]
macro_rules! __impl_ext_for_parachain {
	// entry point: generate ext name
	($name:ident, $runtime:path, $origin:path, $new_ext:expr) => {
		$crate::paste::paste! {
			$crate::__impl_ext_for_parachain!(@impl $name, $runtime, $origin, $new_ext, [<EXT_ $name:upper>]);
		}
	};
	// impl
	(@impl $name:ident, $runtime:path, $origin:path, $new_ext:expr, $ext_name:ident) => {
		thread_local! {
			pub static $ext_name: $crate::RefCell<$crate::TestExternalities>
				= $crate::RefCell::new($new_ext);
		}

		impl $name {
			fn prepare_for_xcmp() {
				$ext_name.with(|v| {
					v.borrow_mut().execute_with(|| {
						use $crate::{Get, Hooks};
						type ParachainSystem = $crate::cumulus_pallet_parachain_system::Pallet<$runtime>;

						let block_number = $crate::frame_system::Pallet::<$runtime>::block_number();
						let para_id = $crate::parachain_info::Pallet::<$runtime>::get();

						let _ = ParachainSystem::set_validation_data(
							<$origin>::none(),
							_hrmp_channel_parachain_inherent_data(para_id.into(), 1),
						);
						// set `AnnouncedHrmpMessagesPerCandidate`
						ParachainSystem::on_initialize(block_number);
					})
				});
			}
		}

		impl $crate::TestExt for $name {
			fn new_ext() -> $crate::TestExternalities {
				$new_ext
			}

			fn reset_ext() {
				$ext_name.with(|v| *v.borrow_mut() = $new_ext);
			}

			fn execute_with<R>(execute: impl FnOnce() -> R) -> R {
				use $crate::{Get, Hooks};
				type ParachainSystem = $crate::cumulus_pallet_parachain_system::Pallet<$runtime>;

				$crate::GLOBAL_RELAY.with(|v| {
					*v.borrow_mut() += 1;
				});

				$ext_name.with(|v| {
					v.borrow_mut().execute_with(|| {
						let para_id = $crate::parachain_info::Pallet::<$runtime>::get();
						$crate::GLOBAL_RELAY.with(|v| {
							let relay_block = *v.borrow();
							let _ = ParachainSystem::set_validation_data(
								<$origin>::none(),
								_hrmp_channel_parachain_inherent_data(para_id.into(), relay_block),
							);
						});
					})
				});

				let r = $ext_name.with(|v| v.borrow_mut().execute_with(execute));

				// send messages if needed
				$ext_name.with(|v| {
					v.borrow_mut().execute_with(|| {
						use sp_runtime::traits::Header as HeaderT;

						let block_number = $crate::frame_system::Pallet::<$runtime>::block_number();
						let mock_header = HeaderT::new(
							0,
							Default::default(),
							Default::default(),
							Default::default(),
							Default::default(),
						);

						// get messages
						ParachainSystem::on_finalize(block_number);
						let collation_info = ParachainSystem::collect_collation_info(&mock_header);

						// send upward messages
						let para_id = $crate::parachain_info::Pallet::<$runtime>::get();
						for msg in collation_info.upward_messages.clone() {
							_Messenger::send_upward_message(para_id.into(), msg);
						}

						// send horizontal messages
						for msg in collation_info.horizontal_messages {
							$crate::GLOBAL_RELAY.with(|v| {
								let relay_block = *v.borrow();
								_Messenger::send_horizontal_messages(
									msg.recipient.into(),
									vec![(para_id.into(), relay_block, msg.data)].into_iter(),
								);
							});
						}

						// clean messages
						ParachainSystem::on_initialize(block_number);
					})
				});

				_process_messages();

				r
			}
		}
	};
}

thread_local! {
	/// Downward messages, each message is: `(to_para_id, [(relay_block_number, msg)])`
	#[allow(clippy::type_complexity)]
	pub static DOWNWARD_MESSAGES: RefCell<VecDeque<(u32, Vec<(RelayBlockNumber, Vec<u8>)>)>>
		= RefCell::new(VecDeque::new());
		#[allow(clippy::type_complexity)]
	/// Downward messages that already processed by parachains, each message is: `(to_para_id, relay_block_number, Vec<u8>)`
	pub static DMP_DONE: RefCell<VecDeque<(u32, RelayBlockNumber, Vec<u8>)>>
		= RefCell::new(VecDeque::new());
	/// Horizontal messages, each message is: `(to_para_id, [(from_para_id, relay_block_number, msg)])`
	#[allow(clippy::type_complexity)]
	pub static HORIZONTAL_MESSAGES: RefCell<VecDeque<(u32, Vec<(ParaId, RelayBlockNumber, Vec<u8>)>)>>
		= RefCell::new(VecDeque::new());
	/// Upward messages, each message is: `(from_para_id, msg)
	pub static UPWARD_MESSAGES: RefCell<VecDeque<(u32, Vec<u8>)>> = RefCell::new(VecDeque::new());
	/// Global incremental relay chain block number
	pub static GLOBAL_RELAY: RefCell<u32> = RefCell::new(1);
}

#[macro_export]
macro_rules! decl_test_network {
	(
		pub struct $name:ident {
			relay_chain = $relay_chain:ty,
			parachains = vec![ $( ($para_id:expr, $parachain:ty), )* ],
		}
	) => {
		pub struct $name;

		impl $name {
			pub fn reset() {
				use $crate::{TestExt, VecDeque};

				<$relay_chain>::reset_ext();
				$( <$parachain>::reset_ext(); )*

				$( <$parachain>::prepare_for_xcmp(); )*

				$crate::DOWNWARD_MESSAGES.with(|b| b.replace(VecDeque::new()));
				$crate::DMP_DONE.with(|b| b.replace(VecDeque::new()));
			}
		}

		fn _para_ids() -> Vec<u32> {
			vec![$( $para_id, )*]
		}

		fn _process_messages() {
			while _has_unprocessed_messages() {
				_process_upward_messages();
				_process_horizontal_messages();
				_process_downward_messages();
			}
		}

		fn _has_unprocessed_messages() -> bool {
			$crate::DOWNWARD_MESSAGES.with(|b| !b.borrow_mut().is_empty())
			|| $crate::HORIZONTAL_MESSAGES.with(|b| !b.borrow_mut().is_empty())
			|| $crate::UPWARD_MESSAGES.with(|b| !b.borrow_mut().is_empty())
		}

		fn _process_downward_messages() {
			use $crate::{DmpMessageHandler, Bounded};
			use polkadot_parachain::primitives::RelayChainBlockNumber;

			while let Some((to_para_id, messages))
				= $crate::DOWNWARD_MESSAGES.with(|b| b.borrow_mut().pop_front()) {
				match to_para_id {
					$(
						$para_id => {
							let mut msg_dedup: Vec<(RelayChainBlockNumber, Vec<u8>)> = Vec::new();
							for m in messages {
								msg_dedup.push((m.0, m.1.clone()));
							}
							msg_dedup.dedup();

							let msgs = msg_dedup.clone().into_iter().filter(|m| {
								!$crate::DMP_DONE.with(|b| b.borrow_mut().contains(&(to_para_id, m.0, m.1.clone())))
							}).collect::<Vec<(RelayChainBlockNumber, Vec<u8>)>>();
							if msgs.len() != 0 {
								<$parachain>::handle_dmp_messages(msgs.clone().into_iter(), $crate::Weight::max_value());
								for m in msgs {
									$crate::DMP_DONE.with(|b| b.borrow_mut().push_back((to_para_id, m.0, m.1)));
								}
							}
						},
					)*
					_ => unreachable!(),
				}
			}
		}

		fn _process_horizontal_messages() {
			use $crate::{XcmpMessageHandler, Bounded};

			while let Some((to_para_id, messages))
				= $crate::HORIZONTAL_MESSAGES.with(|b| b.borrow_mut().pop_front()) {
				let iter = messages.iter().map(|(p, b, m)| (*p, *b, &m[..])).collect::<Vec<_>>().into_iter();
				match to_para_id {
					$(
						$para_id => {
							<$parachain>::handle_xcmp_messages(iter, $crate::Weight::max_value());
						},
					)*
					_ => unreachable!(),
				}
			}
		}

		fn _process_upward_messages() {
			use $crate::{UmpSink, Bounded};
			while let Some((from_para_id, msg)) = $crate::UPWARD_MESSAGES.with(|b| b.borrow_mut().pop_front()) {
				let _ =  <$relay_chain>::process_upward_message(
					from_para_id.into(),
					&msg[..],
					$crate::Weight::max_value(),
				);
			}
		}

		pub struct _Messenger;
		impl _Messenger {
			fn send_downward_messages(to_para_id: u32, iter: impl Iterator<Item = ($crate::RelayBlockNumber, Vec<u8>)>) {
				$crate::DOWNWARD_MESSAGES.with(|b| b.borrow_mut().push_back((to_para_id, iter.collect())));
			}

			fn send_horizontal_messages<
				I: Iterator<Item = ($crate::ParaId, $crate::RelayBlockNumber, Vec<u8>)>,
			>(to_para_id: u32, iter: I) {
				$crate::HORIZONTAL_MESSAGES.with(|b| b.borrow_mut().push_back((to_para_id, iter.collect())));
			}

			fn send_upward_message(from_para_id: u32, msg: Vec<u8>) {
				$crate::UPWARD_MESSAGES.with(|b| b.borrow_mut().push_back((from_para_id, msg)));
			}
		}

		fn _hrmp_channel_parachain_inherent_data(
			para_id: u32,
			relay_parent_number: u32,
		) -> $crate::ParachainInherentData {
			use $crate::cumulus_primitives_core::{relay_chain::HrmpChannelId, AbridgedHrmpChannel};

			let mut sproof = $crate::RelayStateSproofBuilder::default();
			sproof.para_id = para_id.into();

			// egress channel
			let e_index = sproof.hrmp_egress_channel_index.get_or_insert_with(Vec::new);
			for recipient_para_id in &[ $( $para_id, )* ] {
				let recipient_para_id = $crate::ParaId::from(*recipient_para_id);
				if let Err(idx) = e_index.binary_search(&recipient_para_id) {
					e_index.insert(idx, recipient_para_id);
				}

				sproof
					.hrmp_channels
					.entry(HrmpChannelId {
						sender: sproof.para_id,
						recipient: recipient_para_id,
					})
					.or_insert_with(|| AbridgedHrmpChannel {
						max_capacity: 1024,
						max_total_size: 1024 * 1024,
						max_message_size: 1024 * 1024,
						msg_count: 0,
						total_size: 0,
						mqc_head: Option::None,
					});
			}

			let (relay_storage_root, proof) = sproof.into_state_root_and_proof();

			$crate::ParachainInherentData {
				validation_data: $crate::PersistedValidationData {
					parent_head: Default::default(),
					relay_parent_number,
					relay_parent_storage_root: relay_storage_root,
					max_pov_size: Default::default(),
				},
				relay_chain_state: proof,
				downward_messages: Default::default(),
				horizontal_messages: Default::default(),
			}
		}
	};
}
