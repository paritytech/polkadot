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
use xcm::prelude::*;

fn assert_last_event_type<T: Config>(generic_event: <T as Config>::RuntimeEvent) {
	let events = frame_system::Pallet::<T>::events();
	let system_event: <T as frame_system::Config>::RuntimeEvent = generic_event.into();
	// compare to the last event record
	let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
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
	assert_last_event_type::<T>(Event::UpwardMessagesReceived(para, 1, len).into());
}

// Create a message with at least `size` bytes encoded length
fn create_message_min_size<T: Config>(size: u32) -> Vec<u8> {
	// Create a message with an empty remark call to determine the encoding overhead
	let msg_size_empty_transact = VersionedXcm::<T>::from(Xcm::<T>(vec![Transact {
		origin_kind: OriginKind::SovereignAccount,
		require_weight_at_most: Weight::MAX,
		call: frame_system::Call::<T>::remark_with_event { remark: vec![] }.encode().into(),
	}]))
	.encode()
	.len();

	// Create a message with a remark call of just the size required to make the whole encoded message the requested size
	let size = size.saturating_sub(msg_size_empty_transact as u32) as usize;
	let mut remark = Vec::new();
	remark.resize(size, 0u8);
	let msg = VersionedXcm::<T>::from(Xcm::<T>(vec![Transact {
		origin_kind: OriginKind::SovereignAccount,
		require_weight_at_most: Weight::MAX,
		call: frame_system::Call::<T>::remark_with_event { remark }.encode().into(),
	}]))
	.encode();

	assert!(msg.len() >= size);
	msg
}

fn create_message_overweight<T: Config>() -> Vec<u8> {
	// We use a `set_code` Call because it
	let call = frame_system::Call::<T>::set_code { code: vec![] };
	VersionedXcm::<T>::from(Xcm::<T>(vec![Transact {
		origin_kind: OriginKind::Superuser,
		require_weight_at_most: Weight::MAX / 2,
		call: call.encode().into(),
	}]))
	.encode()
}

frame_benchmarking::benchmarks! {
	// NOTE: We are overestimating slightly here.
	// The benchmark is timing this whole function with different message sizes and a NOOP extrinsic to
	// measure the size-dependent weight. But as we use the weight function **in** the benchmarked function we
	// are taking call and control-flow overhead into account twice.
	process_upward_message {
		let s in 0..MAX_UPWARD_MESSAGE_SIZE_BOUND;
		let para = ParaId::from(1978);
		let data = create_message_min_size::<T>(s);
	}: {
		assert!(T::UmpSink::process_upward_message(para, &data[..], Weight::MAX).is_ok());
	}

	clean_ump_after_outgoing {
		// max number of queued messages.
		let count = configuration::ActiveConfig::<T>::get().max_upward_queue_count;
		let host_conf = configuration::ActiveConfig::<T>::get();
		let msg = create_message_min_size::<T>(0);
		// Start with the block number 1. This is needed because should an event be
		// emitted during the genesis block they will be implicitly wiped.
		frame_system::Pallet::<T>::set_block_number(1u32.into());
		// fill the queue, each message has it's own para-id.
		for id in 0..count {
			queue_upward_msg::<T>(&host_conf, ParaId::from(id), msg.clone());
		}
	}: {
		Ump::<T>::clean_ump_after_outgoing(&ParaId::from(0));
	}

	service_overweight {
		let host_conf = configuration::ActiveConfig::<T>::get();
		let para = ParaId::from(1978);
		// The message's weight does not really matter here, as we add service_overweight's
		// max_weight parameter to the extrinsic's weight in the weight calculation.
		// The size of the message influences decoding time, so we create a min-sized message here
		// and take the decoding weight into account by adding it to the extrinsic execution weight
		// in the process_upward_message function.
		let msg = create_message_overweight::<T>();

		// This just makes sure that 0 is not a valid index and we can use it later on.
		let _ = Ump::<T>::service_overweight(RawOrigin::Root.into(), 0, Weight::from_parts(1000, 1000));
		// Start with the block number 1. This is needed because should an event be
		// emitted during the genesis block they will be implicitly wiped.
		frame_system::Pallet::<T>::set_block_number(1u32.into());
		queue_upward_msg::<T>(&host_conf, para, msg.clone());
		Ump::<T>::process_pending_upward_messages();
		assert_last_event_type::<T>(
			Event::OverweightEnqueued(para, upward_message_id(&msg), 0, Weight::zero()).into()
			);
	}: _(RawOrigin::Root, 0, Weight::MAX)
	verify {
		assert_last_event_type::<T>(Event::OverweightServiced(0, Weight::zero()).into());
	}
}

frame_benchmarking::impl_benchmark_test_suite!(
	Ump,
	crate::mock::new_test_ext(crate::ump::tests::GenesisConfigBuilder::default().build()),
	crate::mock::Test
);
