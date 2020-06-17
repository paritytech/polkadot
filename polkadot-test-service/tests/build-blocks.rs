use futures::{pin_mut, select, FutureExt as _};
use polkadot_test_service::*;
use service::TaskType;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

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
	sc_cli::init_logger("");
	let (service_alice, _client_alice, _handles_alice, _base_path_alice) = run_test_node(task_executor(), 27015, "alice", || {
		use polkadot_test_runtime::*;
		//EpochDuration::set(&42);
		//ExpectedBlockTime::set(&6000);
		//panic!("{:?}", ExpectedBlockTime::get());
		//panic!("{:?}", MinimumPeriod::get());
		SlotDuration::set(&2000);
		//MinimumPeriod::set(&1000);
	})
	.unwrap();

	let (service_bob, _client_bob, _handles_bob, _base_path_bob) = run_test_node(task_executor(), 27016, "bob", || {
		use polkadot_test_runtime::*;
		//EpochDuration::set(&42);
		//ExpectedBlockTime::set(&6000);
		//panic!("{:?}", ExpectedBlockTime::get());
		//panic!("{:?}", MinimumPeriod::get());
		SlotDuration::set(&2000);
		//MinimumPeriod::set(&1000);
	})
	.unwrap();

	let t1 = service_alice.fuse();
	let t2 = service_bob.fuse();
	let t3 = async_std::task::sleep(Duration::from_secs(20)).fuse();

	pin_mut!(t1, t2, t3);

	select! {
		_ = t1 => {},
		_ = t2 => {},
		_ = t3 => {},
	}

	assert!(false);
}
