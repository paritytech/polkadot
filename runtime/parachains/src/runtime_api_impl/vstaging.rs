// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Put implementations of functions from staging APIs here.

use crate::{configuration, dmp, hrmp, inclusion, initializer, paras, shared};
use frame_system::pallet_prelude::BlockNumberFor;
use primitives::{
	vstaging::{
		AsyncBackingParams, BackingState, CandidatePendingAvailability, Constraints,
		InboundHrmpLimitations, OutboundHrmpChannelLimitations,
	},
	Id as ParaId,
};
use sp_std::prelude::*;

/// Implementation for `StagingParaBackingState` function from the runtime API
pub fn backing_state<T: initializer::Config>(
	para_id: ParaId,
) -> Option<BackingState<T::Hash, BlockNumberFor<T>>> {
	let config = <configuration::Pallet<T>>::config();
	// Async backing is only expected to be enabled with a tracker capacity of 1.
	// Subsequent configuration update gets applied on new session, which always
	// clears the buffer.
	//
	// Thus, minimum relay parent is ensured to have asynchronous backing enabled.
	let now = <frame_system::Pallet<T>>::block_number();
	let min_relay_parent_number = <shared::Pallet<T>>::allowed_relay_parents()
		.hypothetical_earliest_block_number(now, config.async_backing_params.allowed_ancestry_len);

	let required_parent = <paras::Pallet<T>>::para_head(para_id)?;
	let validation_code_hash = <paras::Pallet<T>>::current_code_hash(para_id)?;

	let upgrade_restriction = <paras::Pallet<T>>::upgrade_restriction_signal(para_id);
	let future_validation_code =
		<paras::Pallet<T>>::future_code_upgrade_at(para_id).and_then(|block_num| {
			// Only read the storage if there's a pending upgrade.
			Some(block_num).zip(<paras::Pallet<T>>::future_code_hash(para_id))
		});

	let (ump_msg_count, ump_total_bytes) =
		<inclusion::Pallet<T>>::relay_dispatch_queue_size(para_id);
	let ump_remaining = config.max_upward_queue_count - ump_msg_count;
	let ump_remaining_bytes = config.max_upward_queue_size - ump_total_bytes;

	let dmp_remaining_messages = <dmp::Pallet<T>>::dmq_contents(para_id)
		.into_iter()
		.map(|msg| msg.sent_at)
		.collect();

	let valid_watermarks = <hrmp::Pallet<T>>::valid_watermarks(para_id);
	let hrmp_inbound = InboundHrmpLimitations { valid_watermarks };
	let hrmp_channels_out = <hrmp::Pallet<T>>::outbound_remaining_capacity(para_id)
		.into_iter()
		.map(|(para, (messages_remaining, bytes_remaining))| {
			(para, OutboundHrmpChannelLimitations { messages_remaining, bytes_remaining })
		})
		.collect();

	let constraints = Constraints {
		min_relay_parent_number,
		max_pov_size: config.max_pov_size,
		max_code_size: config.max_code_size,
		ump_remaining,
		ump_remaining_bytes,
		max_ump_num_per_candidate: config.max_upward_message_num_per_candidate,
		dmp_remaining_messages,
		hrmp_inbound,
		hrmp_channels_out,
		max_hrmp_num_per_candidate: config.hrmp_max_message_num_per_candidate,
		required_parent,
		validation_code_hash,
		upgrade_restriction,
		future_validation_code,
	};

	let pending_availability = {
		// Note: the API deals with a `Vec` as it is future-proof for cases
		// where there may be multiple candidates pending availability at a time.
		// But at the moment only one candidate can be pending availability per
		// parachain.
		crate::inclusion::PendingAvailability::<T>::get(&para_id)
			.and_then(|pending| {
				let commitments =
					crate::inclusion::PendingAvailabilityCommitments::<T>::get(&para_id);
				commitments.map(move |c| (pending, c))
			})
			.map(|(pending, commitments)| {
				CandidatePendingAvailability {
					candidate_hash: pending.candidate_hash(),
					descriptor: pending.candidate_descriptor().clone(),
					commitments,
					relay_parent_number: pending.relay_parent_number(),
					max_pov_size: constraints.max_pov_size, // assume always same in session.
				}
			})
			.into_iter()
			.collect()
	};

	Some(BackingState { constraints, pending_availability })
}

/// Implementation for `StagingAsyncBackingParams` function from the runtime API
pub fn async_backing_params<T: configuration::Config>() -> AsyncBackingParams {
	<configuration::Pallet<T>>::config().async_backing_params
}
