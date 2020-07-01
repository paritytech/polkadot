use async_std::task::{block_on, sleep};
use futures::{pin_mut, select, FutureExt as _};
use polkadot_primitives::parachain::{Info, Scheduling};
use polkadot_runtime_common::{parachains, registrar, BlockHashCount};
use polkadot_test_runtime::{RestrictFunctionality, Runtime, SignedExtra, SignedPayload, VERSION};
use polkadot_test_service::*;
use sp_arithmetic::traits::SaturatedConversion;
use sp_blockchain::HeaderBackend;
use sp_keyring::Sr25519Keyring::Alice;
use sp_runtime::{codec::Encode, generic};
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
		let current_block_hash = alice.client.info().best_hash;
		let current_block = alice.client.info().best_number.saturated_into();
		let genesis_block = alice.client.hash(0).unwrap().unwrap();
		let nonce = 0;
		let period = BlockHashCount::get()
			.checked_next_power_of_two()
			.map(|c| c / 2)
			.unwrap_or(2) as u64;
		let tip = 0;
		let extra: SignedExtra = (
			RestrictFunctionality,
			frame_system::CheckSpecVersion::<Runtime>::new(),
			frame_system::CheckTxVersion::<Runtime>::new(),
			frame_system::CheckGenesis::<Runtime>::new(),
			frame_system::CheckEra::<Runtime>::from(generic::Era::mortal(period, current_block)),
			frame_system::CheckNonce::<Runtime>::from(nonce),
			frame_system::CheckWeight::<Runtime>::new(),
			pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
			registrar::LimitParathreadCommits::<Runtime>::new(),
			parachains::ValidateDoubleVoteReports::<Runtime>::new(),
		);
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
		let raw_payload = SignedPayload::from_raw(
			function.clone(),
			extra.clone(),
			((),
			VERSION.spec_version,
			VERSION.transaction_version,
			genesis_block,
			current_block_hash,
			(), (), (), (), ()),
		);
		let signature = raw_payload.using_encoded(|e| Alice.sign(e));
		let extrinsic = polkadot_test_runtime::UncheckedExtrinsic::new_signed(
			function.clone(),
			polkadot_test_runtime::Address::Id(Alice.public().into()),
			polkadot_primitives::Signature::Sr25519(signature.clone()),
			extra.clone(),
		);

		let (res, _mem, _rx) = alice.send_transaction(extrinsic.into()).await;

		assert!(res.is_some(), "return value expected");
		let json = serde_json::from_str::<serde_json::Value>(res.unwrap().as_str()).expect("valid JSON");
		assert!(json.is_object(), "JSON is an object");
		let object = json.as_object().unwrap();
		assert!(object.contains_key("jsonrpc"), "key jsonrpc exists");
		let result = object.get("result");
		assert!(result.is_some(), "key result exists");
		let result = result.unwrap();
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
