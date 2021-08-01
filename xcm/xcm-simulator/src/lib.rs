// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Test kit to simulate cross-chain message passing and XCM execution

pub use codec::Encode;
pub use paste;

pub use frame_support::{traits::Get, weights::Weight};
pub use sp_io::TestExternalities;
pub use sp_std::{cell::RefCell, marker::PhantomData};

pub use cumulus_pallet_dmp_queue;
pub use cumulus_pallet_xcmp_queue;
pub use cumulus_primitives_core;

pub use polkadot_parachain::primitives::Id as ParaId;
pub use polkadot_runtime_parachains::{dmp, ump};
pub use xcm::{v0::prelude::*, VersionedXcm};
pub use xcm_executor::XcmExecutor;

mod traits;
pub use traits::{HandleDmpMessage, HandleUmpMessage, HandleXcmpMessage, TestExt};

pub enum MessageKind {
	Ump,
	Dmp,
	Xcmp,
}

pub fn encode_xcm(message: Xcm<()>, message_kind: MessageKind) -> Vec<u8> {
	match message_kind {
		MessageKind::Ump | MessageKind::Dmp => VersionedXcm::<()>::from(message).encode(),
		MessageKind::Xcmp => {
			let fmt = cumulus_pallet_xcmp_queue::XcmpMessageFormat::ConcatenatedVersionedXcm;
			let mut outbound = fmt.encode();

			let encoded = VersionedXcm::<()>::from(message).encode();
			outbound.extend_from_slice(&encoded[..]);
			outbound
		}
	}
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

		$crate::__impl_ext!($name, $new_ext);

		impl $crate::HandleUmpMessage for $name {
			fn handle_ump_message(from: $crate::ParaId, msg: &[u8], max_weight: $crate::Weight) {
				use $crate::ump::UmpSink;
				use $crate::TestExt;

				Self::execute_with(|| {
					let _ = $crate::ump::XcmSink::<$crate::XcmExecutor<$xcm_config>, $runtime>::process_upward_message(
						from, msg, max_weight,
					);
				});
			}
		}
	};
}

#[macro_export]
macro_rules! decl_test_parachain {
	(
		pub struct $name:ident {
			Runtime = $runtime:path,
			new_ext = $new_ext:expr,
		}
	) => {
		pub struct $name;

		$crate::__impl_ext!($name, $new_ext);

		impl $crate::HandleXcmpMessage for $name {
			fn handle_xcmp_message(from: $crate::ParaId, at_relay_block: u32, msg: &[u8], max_weight: $crate::Weight) {
				use $crate::cumulus_primitives_core::XcmpMessageHandler;
				use $crate::TestExt;

				$name::execute_with(|| {
					$crate::cumulus_pallet_xcmp_queue::Pallet::<$runtime>::handle_xcmp_messages(
						vec![(from, at_relay_block, msg)].into_iter(),
						max_weight,
					);
				});
			}
		}

		impl $crate::HandleDmpMessage for $name {
			fn handle_dmp_message(at_relay_block: u32, msg: Vec<u8>, max_weight: $crate::Weight) {
				use $crate::cumulus_primitives_core::DmpMessageHandler;
				use $crate::TestExt;

				$name::execute_with(|| {
					$crate::cumulus_pallet_dmp_queue::Pallet::<$runtime>::handle_dmp_messages(
						vec![(at_relay_block, msg)].into_iter(),
						max_weight,
					);
				});
			}
		}
	};
}

#[macro_export]
macro_rules! __impl_ext {
	// entry point: generate ext name
	($name:ident, $new_ext:expr) => {
		$crate::paste::paste! {
			$crate::__impl_ext!(@impl $name, $new_ext, [<EXT_ $name:upper>]);
		}
	};
	// impl
	(@impl $name:ident, $new_ext:expr, $ext_name:ident) => {
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
				$ext_name.with(|v| v.borrow_mut().execute_with(execute))
			}
		}
	};
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
				use $crate::TestExt;

				<$relay_chain>::reset_ext();
				$( <$parachain>::reset_ext(); )*
			}
		}

		/// XCM router for parachain.
		pub struct ParachainXcmRouter<T>($crate::PhantomData<T>);

		impl<T: $crate::Get<$crate::ParaId>> $crate::SendXcm for ParachainXcmRouter<T> {
			fn send_xcm(destination: $crate::MultiLocation, message: $crate::Xcm<()>) -> $crate::XcmResult {
				use $crate::{HandleUmpMessage, HandleXcmpMessage};

				match destination {
					$crate::X1($crate::Parent) => {
						let encoded = $crate::encode_xcm(message, $crate::MessageKind::Ump);
						<$relay_chain>::handle_ump_message(T::get(), &encoded[..], $crate::Weight::max_value());
						// TODO: update max weight
						Ok(())
					},
					$(
						$crate::X2($crate::Parent, $crate::Parachain(id)) if id == $para_id => {
							let encoded = $crate::encode_xcm(message, $crate::MessageKind::Xcmp);
							// TODO: update max weight; update `at_relay_block`
							<$parachain>::handle_xcmp_message(T::get(), 1, &encoded[..], $crate::Weight::max_value());
							Ok(())
						},
					)*
					_ => Err($crate::XcmError::CannotReachDestination(destination, message)),
				}
			}
		}

		/// XCM router, only sends DMP messages.
		pub struct RelayChainXcmRouter;
		impl $crate::SendXcm for RelayChainXcmRouter {
			fn send_xcm(destination: $crate::MultiLocation, message: $crate::Xcm<()>) -> $crate::XcmResult {
				use $crate::HandleDmpMessage;

				match destination {
					$(
						$crate::X1($crate::Parachain(id)) if id == $para_id => {
							let encoded = $crate::encode_xcm(message, $crate::MessageKind::Dmp);
							// TODO: update max weight; update `at_relay_block`
							<$parachain>::handle_dmp_message(1, encoded, $crate::Weight::max_value());
							Ok(())
						},
					)*
					_ => Err($crate::XcmError::SendFailed("Only sends to children parachain.")),
				}
			}
		}
	};
}

