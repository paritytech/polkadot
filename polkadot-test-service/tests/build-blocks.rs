use polkadot_test_service::*;
use std::pin::Pin;
use std::sync::Arc;
use futures::{FutureExt as _, select, pin_mut};
use std::time::Duration;
use service::TaskType;

fn task_executor() -> Arc<dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync> {
	Arc::new(move |fut: Pin<Box<dyn futures::Future<Output = ()> + Send>>, _| { async_std::task::spawn(fut.unit_error()); })
}

#[async_std::test]
async fn ensure_test_service_build_blocks() {
	let task_executor = task_executor();

	let (service, _client, _handles, _base_path) = run_test_node(
		task_executor,
		27015,
		|| {
			use polkadot_test_runtime::*;
			EpochDuration::set(&42);
			//panic!("{:?}", EpochDuration::get());
		},
	).unwrap();

	let t1 = service.fuse();
	let t2 = async_std::task::sleep(Duration::from_secs(10)).fuse();

	pin_mut!(t1, t2);

	select! {
		_ = t1 => {},
		_ = t2 => {},
	}

	assert!(false);
}
