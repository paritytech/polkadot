use async_std::task::{block_on, sleep};
use futures::{pin_mut, select, FutureExt as _};
use polkadot_primitives::parachain::{Info, Scheduling};
use polkadot_runtime_common::{registrar};
use polkadot_test_service::*;
use sp_keyring::Sr25519Keyring::Alice;
use std::time::Duration;

static INTEGRATION_TEST_ALLOWED_TIME: Option<&str> = option_env!("INTEGRATION_TEST_ALLOWED_TIME");

#[test]
fn send_transaction_actually_work() {
	let mut alice = run_test_node(
		(|fut, _| {
			async_std::task::spawn(fut);
		})
		.into(),
		Alice,
		|| {},
		Vec::new(),
	)
	.unwrap();
	let t1 = sleep(Duration::from_secs(
		INTEGRATION_TEST_ALLOWED_TIME
			.and_then(|x| x.parse().ok())
			.unwrap_or(600),
	))
	.fuse();
	let t2 = async {
		let wasm = vec![0_u8; 32];
		let genesis_state = b"0x0x000000000000000000000000000000000000000000000000000000000000000000eb21415d4113e9bb8c\
							0c3fa5533d873c439e94960c56f4c1dd1105ddc6b7b2e903170a2e7597b7b7e3d84c05391d139a62b157e78786\
							d8c082f29dcf4c11131400".to_vec();
		let function = polkadot_test_runtime::Call::Sudo(pallet_sudo::Call::sudo(Box::new(
			polkadot_test_runtime::Call::Registrar(registrar::Call::register_para(
				100.into(),
				Info {
					scheduling: Scheduling::Always,
				},
				wasm.into(),
				genesis_state.into(),
			)),
		)));
		let (res, _mem, _rx) = alice.call_function(function, Alice).await;

		let res = res.expect("return value expected");
		let json = serde_json::from_str::<serde_json::Value>(res.as_str()).expect("valid JSON");
		let object = json.as_object().expect("JSON is an object");
		assert!(object.contains_key("jsonrpc"), "key jsonrpc exists");
		let result = object.get("result");
		let result = result.expect("key result exists");
		assert_eq!(result.as_str().map(|x| x.starts_with("0x")), Some(true), "result starts with 0x");

		alice.task_manager.terminate();
	}
	.fuse();

	block_on(async move {
		pin_mut!(t1, t2);

		select! {
			_ = t1 => {
				panic!("the test took too long, maybe no blocks have been produced");
			},
			_ = t2 => {},
		}
	});
}
