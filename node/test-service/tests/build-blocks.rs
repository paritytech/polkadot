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

use tokio::{time::delay_for as sleep, task::spawn};
use futures::{future, pin_mut, select, FutureExt as _};
use polkadot_test_service::*;
use service::TaskExecutor;
use sp_keyring::Sr25519Keyring;
use std::time::Duration;

static INTEGRATION_TEST_ALLOWED_TIME: Option<&str> = option_env!("INTEGRATION_TEST_ALLOWED_TIME");

#[tokio::test]
async fn ensure_test_service_build_blocks() {
	let task_executor: TaskExecutor = (move |fut, _| {
		spawn(fut).map(|_| ())
	})
	.into();
	let mut alice = run_test_node(
		task_executor.clone(),
		Sr25519Keyring::Alice,
		|| {},
		Vec::new(),
	);
	let mut bob = run_test_node(
		task_executor.clone(),
		Sr25519Keyring::Bob,
		|| {},
		vec![alice.addr.clone()],
	);
	let t1 = sleep(Duration::from_secs(
		INTEGRATION_TEST_ALLOWED_TIME
			.and_then(|x| x.parse().ok())
			.unwrap_or(600),
	))
	.fuse();
	let t2 = async {
		{
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

		alice.task_manager.terminate();
		bob.task_manager.terminate();
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
