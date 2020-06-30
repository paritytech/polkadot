use async_std::task::sleep;
use futures::{future, pin_mut, select, FutureExt as _};
use polkadot_test_service::*;
use service::TaskExecutor;
use sp_keyring::Sr25519Keyring;
use std::time::Duration;

static INTEGRATION_TEST_ALLOWED_TIME: Option<&str> = option_env!("INTEGRATION_TEST_ALLOWED_TIME");

#[async_std::test]
async fn ensure_test_service_build_blocks() {
	let task_executor: TaskExecutor = (|fut, _| {
		async_std::task::spawn(fut);
	})
	.into();
	let t1 = sleep(Duration::from_secs(
		INTEGRATION_TEST_ALLOWED_TIME
			.and_then(|x| x.parse().ok())
			.unwrap_or(600),
	))
	.fuse();
	let t2 = async {
		let mut alice = run_test_node(
			task_executor.clone(),
			Sr25519Keyring::Alice,
			|| {},
			Vec::new(),
		)
		.unwrap();

		let mut bob = run_test_node(
			task_executor.clone(),
			Sr25519Keyring::Bob,
			|| {},
			vec![alice.multiaddr_with_peer_id.clone()],
		)
		.unwrap();

		let t1 = future::join(alice.wait_for_blocks(3), bob.wait_for_blocks(3)).fuse();
		let t2 = alice.task_manager.future().fuse();
		let t3 = bob.task_manager.future().fuse();

		pin_mut!(t1, t2, t3);

		select! {
			_ = t1 => {},
			_ = t2 => panic!("service Alice failed"),
			_ = t3 => panic!("service Bob failed"),
		}
	}
	.fuse();

	pin_mut!(t1, t2);

	select! {
		_ = t1 => {
			panic!("the test took too long, maybe no blocks have been produced");
		},
		_ = t2 => {},
	}
}
