use super::*;
use futures::{pin_mut, executor::block_on};
use polkadot_primitives::v1::{CandidateHash, OccupiedCore};
use polkadot_node_subsystem::messages::AllMessages;

fn occupied_core(para_id: u32, candidate_hash: CandidateHash) -> CoreState {
    CoreState::Occupied(OccupiedCore {
        group_responsible: para_id.into(),
        next_up_on_available: None,
        occupied_since: 100_u32,
        time_out_at: 200_u32,
        next_up_on_time_out: None,
        availability: Default::default(),
        candidate_hash,
        candidate_descriptor: Default::default(),
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
        ).fuse();
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
