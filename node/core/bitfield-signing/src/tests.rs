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
use futures::{executor::block_on, pin_mut, StreamExt};
use polkadot_node_subsystem::messages::AllMessages;
use polkadot_primitives::v2::{CandidateHash, OccupiedCore};
use test_helpers::dummy_candidate_descriptor;

fn occupied_core(para_id: u32, candidate_hash: CandidateHash) -> CoreState {
	CoreState::Occupied(OccupiedCore {
		group_responsible: para_id.into(),
		next_up_on_available: None,
		occupied_since: 100_u32,
		time_out_at: 200_u32,
		next_up_on_time_out: None,
		availability: Default::default(),
		candidate_hash,
		candidate_descriptor: dummy_candidate_descriptor(Hash::zero()),
	})
}

#[test]
fn construct_availability_bitfield_works() {
	block_on(async move {
		let relay_parent = Hash::default();
		let validator_index = ValidatorIndex(1u32);

		let (mut sender, mut receiver) = polkadot_node_subsystem_test_helpers::sender_receiver();
		let future = construct_availability_bitfield(
			relay_parent,
			&jaeger::Span::Disabled,
			validator_index,
			&mut sender,
		)
		.fuse();
		pin_mut!(future);

		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));

		loop {
			futures::select! {
				m = receiver.next() => match m.unwrap() {
					AllMessages::RuntimeApi(
						RuntimeApiMessage::Request(rp, RuntimeApiRequest::AvailabilityCores(tx)),
					) => {
						assert_eq!(relay_parent, rp);
						tx.send(Ok(vec![CoreState::Free, occupied_core(1, hash_a), occupied_core(2, hash_b)])).unwrap();
					}
					AllMessages::AvailabilityStore(
						AvailabilityStoreMessage::QueryChunkAvailability(c_hash, vidx, tx),
					) => {
						assert_eq!(validator_index, vidx);

						tx.send(c_hash == hash_a).unwrap();
					},
					o => panic!("Unknown message: {:?}", o),
				},
				r = future => match r {
					Ok(r) => {
						assert!(!r.0.get(0).unwrap());
						assert!(r.0.get(1).unwrap());
						assert!(!r.0.get(2).unwrap());
						break
					},
					Err(e) => panic!("Failed: {:?}", e),
				},
			}
		}
	});
}
