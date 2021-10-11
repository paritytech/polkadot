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

use super::{Pallet as Ump, *};
use frame_system::RawOrigin;
use sp_runtime::Perbill;
use xcm::prelude::*;

fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
	let events = frame_system::Pallet::<T>::events();
	let system_event: <T as frame_system::Config>::Event = generic_event.into();
	// compare to the last event record
	let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
	log::warn!("Last event: {:?}", event);
	assert_eq!(event, &system_event);
}

fn assert_last_event_type<T: Config>(generic_event: <T as Config>::Event) {
	let events = frame_system::Pallet::<T>::events();
	let system_event: <T as frame_system::Config>::Event = generic_event.into();
	// compare to the last event record
	let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
	log::warn!("Last event: {:?}", event);
	assert_eq!(sp_std::mem::discriminant(event), sp_std::mem::discriminant(&system_event));
}

fn queue_upward_msg<T: Config>(
	host_conf: &HostConfiguration<T::BlockNumber>,
	para: ParaId,
	msg: UpwardMessage,
) {
	let len = msg.len() as u32;
	let msgs = vec![msg];
	Ump::<T>::check_upward_messages(host_conf, para, &msgs).unwrap();
	let _ = Ump::<T>::receive_upward_messages(para, msgs);
	assert_last_event::<T>(Event::UpwardMessagesReceived(para, 1, len).into());
}

fn create_message<T: Config>(weight: u64) -> Vec<u8> {
	let max_weight = T::BlockWeights::get().max_block;
	let call =
		frame_system::Call::<T>::fill_block { ratio: Perbill::from_rational(weight, max_weight) };
	VersionedXcm::<T>::from(Xcm::<T>(vec![Transact {
		origin_type: OriginKind::SovereignAccount,
		require_weight_at_most: weight,
		call: call.encode().into(),
	}]))
	.encode()
}

frame_benchmarking::benchmarks! {
	service_overweight {
		let host_conf = configuration::ActiveConfig::<T>::get();
		let weight = host_conf.ump_max_individual_weight + 1;
		let para = ParaId::from(1978);
		let msg = create_message::<T>(weight.into());

		let _ = Ump::<T>::service_overweight(RawOrigin::Root.into(), 0, 1000);
		// Start with the block number 1. This is needed because should an event be
		// emitted during the genesis block they will be implicitly wiped.
		frame_system::Pallet::<T>::set_block_number(1u32.into());
		queue_upward_msg::<T>(&host_conf, para, msg.clone());
		Ump::<T>::process_pending_upward_messages();
		assert_last_event_type::<T>(Event::OverweightEnqueued(para, upward_message_id(&msg), 0, 0).into());
	}: _(RawOrigin::Root, 0, Weight::MAX)
	verify {
		assert_last_event_type::<T>(Event::OverweightServiced(0, 0).into());
	}
}

frame_benchmarking::impl_benchmark_test_suite!(
	Ump,
	crate::mock::new_test_ext(Default::default()).build(),
	crate::mock::Test
);
