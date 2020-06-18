use futures::{future, pin_mut, select, FutureExt as _, StreamExt};
use polkadot_test_service::*;
use service::{AbstractService, TaskType, config::MultiaddrWithPeerId};
use std::pin::Pin;
use std::sync::Arc;
use sp_keyring::Sr25519Keyring;
use sc_client_api::client::BlockchainEvents;
use std::collections::HashSet;
use polkadot_primitives::Block as RelayBlock;

fn task_executor(
) -> Arc<dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync> {
	Arc::new(
		move |fut: Pin<Box<dyn futures::Future<Output = ()> + Send>>, _| {
			async_std::task::spawn(fut.unit_error());
		},
	)
}

#[async_std::test]
async fn ensure_test_service_build_blocks() {
	let ensure_blocks_created = |client: Arc<dyn BlockchainEvents<RelayBlock>>| {
		async move {
			let mut import_notification_stream = client.import_notification_stream();
			let mut blocks = HashSet::new();

			while let Some(notification) = import_notification_stream.next().await {
				blocks.insert(notification.hash);
				if blocks.len() == 3 {
					break;
				}
			}
		}
	};

	sc_cli::init_logger("");
	let (service_alice, client_alice, _handles_alice, _base_path_alice) = run_test_node(
		task_executor(),
		Sr25519Keyring::Alice,
		|| {
			use polkadot_test_runtime::*;
			SlotDuration::set(&2000);
		},
		Vec::new(),
	)
	.await
	.unwrap();

	let (service_bob, client_bob, _handles_bob, _base_path_bob) = run_test_node(
		task_executor(),
		Sr25519Keyring::Bob,
		|| {
			use polkadot_test_runtime::*;
			SlotDuration::set(&2000);
		},
		vec![
			{
				let network = service_alice.network();

				MultiaddrWithPeerId {
					multiaddr: network.listen_addresses().await.remove(0),
					peer_id: network.local_peer_id().clone(),
				}
			},
		],
	)
	.await
	.unwrap();

	let t1 = service_alice.fuse();
	let t2 = service_bob.fuse();
	let t3 = future::join(
		ensure_blocks_created(client_alice),
		ensure_blocks_created(client_bob),
	).fuse();

	pin_mut!(t1, t2, t3);

	select! {
		_ = t1 => panic!("service Alice failed"),
		_ = t2 => panic!("service Bob failed"),
		_ = t3 => {},
	}
}
