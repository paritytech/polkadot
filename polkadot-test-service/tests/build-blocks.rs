use async_std::task::sleep;
use futures::{future, pin_mut, select, FutureExt as _};
use polkadot_test_service::*;
use sp_keyring::Sr25519Keyring;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use substrate_test_client::ClientBlockImportExt;

static INTEGRATION_TEST_ALLOWED_TIME: Option<&str> = option_env!("INTEGRATION_TEST_ALLOWED_TIME");

fn task_executor(
) -> Arc<dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, service::TaskType) + Send + Sync>
{
	Arc::new(
		move |fut: Pin<Box<dyn futures::Future<Output = ()> + Send>>, _| {
			async_std::task::spawn(fut.unit_error());
		},
	)
}

#[async_std::test]
async fn ensure_test_service_build_blocks() {
	let t1 = sleep(Duration::from_secs(
		INTEGRATION_TEST_ALLOWED_TIME
			.and_then(|x| x.parse().ok())
			.unwrap_or(600),
	))
	.fuse();
	let t2 = async {
		let alice = run_test_node(
			task_executor(),
			Sr25519Keyring::Alice,
			|| {},
			Vec::new(),
		)
		.unwrap();

		let bob = run_test_node(
			task_executor(),
			Sr25519Keyring::Bob,
			|| {},
			vec![alice.multiaddr_with_peer_id.clone()],
		)
		.unwrap();

		alice.client.import((), ()).unwrap(); // TODO

		let t1 = future::join(alice.wait_for_blocks(3), bob.wait_for_blocks(3)).fuse();
		let t2 = alice.service.fuse();
		let t3 = bob.service.fuse();

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
