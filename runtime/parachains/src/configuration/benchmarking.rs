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

use crate::configuration::*;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use sp_runtime::traits::One;

benchmarks! {
	set_validation_upgrade_frequency {}: _(RawOrigin::Root, One::one())

	set_validation_upgrade_delay {}: _(RawOrigin::Root, One::one())

	set_code_retention_period {}: _(RawOrigin::Root, One::one())

	set_max_code_size {}: _(RawOrigin::Root, 1024)

	set_max_pov_size {}: _(RawOrigin::Root, 1024)

	set_max_head_data_size {}: _(RawOrigin::Root, 1024)

	set_parathread_cores {}: _(RawOrigin::Root, 5)

	set_parathread_retries {}: _(RawOrigin::Root, 10)

	set_group_rotation_frequency {}: _(RawOrigin::Root, One::one())

	set_chain_availability_period {}: _(RawOrigin::Root, One::one())

	set_thread_availability_period {}: _(RawOrigin::Root, One::one())

	set_scheduling_lookahead {}: _(RawOrigin::Root, 10)

	set_max_validators_per_core {}: _(RawOrigin::Root, Some(10))

	set_max_validators {}: _(RawOrigin::Root, Some(300))

	set_dispute_period {}: _(RawOrigin::Root, 10)

	set_dispute_post_conclusion_acceptance_period {}: _(RawOrigin::Root, One::one())

	set_dispute_max_spam_slots {}: _(RawOrigin::Root, 50)

	set_dispute_conclusion_by_time_out_period {}: _(RawOrigin::Root, One::one())

	set_no_show_slots {}: _(RawOrigin::Root, 10)

	set_n_delay_tranches {}: _(RawOrigin::Root, 10)

	set_zeroth_delay_tranche_width {}: _(RawOrigin::Root, 10)

	set_needed_approvals {}: _(RawOrigin::Root, 5)

	set_relay_vrf_modulo_samples {}: _(RawOrigin::Root, 10)

	set_max_upward_queue_count {}: _(RawOrigin::Root, 3)

	set_max_upward_queue_size {}: _(RawOrigin::Root, 10)

	set_max_downward_message_size {}: _(RawOrigin::Root, 1024)

	set_ump_service_total_weight {}: _(RawOrigin::Root, 3_000_000)

	set_max_upward_message_size {}: _(RawOrigin::Root, 1024)

	set_max_upward_message_num_per_candidate {}: _(RawOrigin::Root, 10)

	set_hrmp_sender_deposit {}: _(RawOrigin::Root, 100)

	set_hrmp_recipient_deposit {}: _(RawOrigin::Root, 100)

	set_hrmp_channel_max_capacity {}: _(RawOrigin::Root, 10)

	set_hrmp_channel_max_total_size {}: _(RawOrigin::Root, 100)

	set_hrmp_max_parachain_inbound_channels {}: _(RawOrigin::Root, 10)

	set_hrmp_max_parathread_inbound_channels {}: _(RawOrigin::Root, 10)

	set_hrmp_channel_max_message_size {}: _(RawOrigin::Root, 1024)

	set_hrmp_max_parachain_outbound_channels {}: _(RawOrigin::Root, 10)

	set_hrmp_max_parathread_outbound_channels {}: _(RawOrigin::Root, 10)

	set_hrmp_max_message_num_per_candidate {}: _(RawOrigin::Root, 10)
}

impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(Default::default()), crate::mock::Test);
