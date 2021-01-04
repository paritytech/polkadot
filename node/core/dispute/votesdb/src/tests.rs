
use super::*;
use bitvec::bitvec;
use futures::executor;
use maplit::hashmap;
use polkadot_primitives::v1::{Signed, ValidatorPair, AvailabilityBitfield};
use polkadot_node_subsystem_test_helpers::make_subsystem_context;
use polkadot_node_subsystem_util::TimeoutExt;
use sp_core::crypto::Pair;
use std::time::Duration;
use assert_matches::assert_matches;
use polkadot_node_network_protocol::ObservedRole;

macro_rules! view {
	( $( $hash:expr ),* $(,)? ) => [
		View(vec![ $( $hash.clone() ),* ])
	];
}


macro_rules! launch {
	($fut:expr) => {
		$fut
		.timeout(Duration::from_millis(10))
		.await
		.expect("10ms is more than enough for sending messages.")
		.expect("Error values should really never occur.")
	};
}


fn init_state(view: View, relay_parent: Hash) -> (ProtocolState, SigningContext, ValidatorPair) {
	let mut state = ProtocolState::default();

	let (validator_pair, _seed) = ValidatorPair::generate();
	let validator = validator_pair.public();

	let signing_context = SigningContext {
		session_index: 1,
		parent_hash: relay_parent.clone(),
	};

	unimplemented!("TODO");

	(state, signing_context, validator_pair)
}

#[test]
fn stance() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();


	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));

	let db = VotesDB::new_in_memory(store, Default::default());

	let (mut state, signing_context, validator_pair) = init_state(view![hash_a, hash_b], hash_a.clone());

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) =
		make_subsystem_context::<VotesDbMessage, _>(pool);

	executor::block_on(async move {
		// launch!();
		// assert_matches!();

		store_vote(ctx, vote).await;
	});
}
