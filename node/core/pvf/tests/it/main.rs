// Copyright 2021 Parity Technologies (UK) Ltd.
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

use polkadot_node_core_pvf::{Pvf, ValidationHost, start, Config, InvalidCandidate, ValidationError};
use polkadot_parachain::primitives::{BlockData, ValidationParams, ValidationResult};
use parity_scale_codec::Encode as _;
use async_std::sync::Mutex;

mod adder;
mod worker_common;

const PUPPET_EXE: &str = env!("CARGO_BIN_EXE_puppet_worker");

struct TestHost {
	_cache_dir: tempfile::TempDir,
	host: Mutex<ValidationHost>,
}

impl TestHost {
	fn new() -> Self {
		Self::new_with_config(|_| ())
	}

	fn new_with_config<F>(f: F) -> Self
	where
		F: FnOnce(&mut Config),
	{
		let cache_dir = tempfile::tempdir().unwrap();
		let program_path = std::path::PathBuf::from(PUPPET_EXE);
		let mut config = Config::new(cache_dir.path().to_owned(), program_path);
		f(&mut config);
		let (host, task) = start(config);
		let _ = async_std::task::spawn(task);
		Self {
			_cache_dir: cache_dir,
			host: Mutex::new(host),
		}
	}

	async fn validate_candidate(
		&self,
		code: &[u8],
		params: ValidationParams,
	) -> Result<ValidationResult, ValidationError> {
		let (result_tx, result_rx) = futures::channel::oneshot::channel();

		let code = sp_maybe_compressed_blob::decompress(code, 16 * 1024 * 1024).expect("Compression works");

		self.host
			.lock()
			.await
			.execute_pvf(
				Pvf::from_code(code.into()),
				params.encode(),
				polkadot_node_core_pvf::Priority::Normal,
				result_tx,
			)
			.await
			.unwrap();
		result_rx.await.unwrap()
	}
}

#[async_std::test]
async fn terminates_on_timeout() {
	let host = TestHost::new();

	let result = host
		.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
		)
		.await;

	match result {
		Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout)) => {}
		r => panic!("{:?}", r),
	}
}

#[async_std::test]
async fn parallel_execution() {
	let host = TestHost::new();
	let execute_pvf_future_1 = host.validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_parent_number: 1,
			relay_parent_storage_root: Default::default(),
		},
	);
	let execute_pvf_future_2 = host.validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_parent_number: 1,
			relay_parent_storage_root: Default::default(),
		},
	);

	let start = std::time::Instant::now();
	let (_, _) = futures::join!(execute_pvf_future_1, execute_pvf_future_2);

	// total time should be < 2 x EXECUTION_TIMEOUT_SEC
	const EXECUTION_TIMEOUT_SEC: u64 = 3;
	assert!(
		std::time::Instant::now().duration_since(start)
			< std::time::Duration::from_secs(EXECUTION_TIMEOUT_SEC * 2)
	);
}

#[async_std::test]
async fn execute_queue_doesnt_stall_if_workers_died() {
	let host = TestHost::new_with_config(|cfg| {
		assert_eq!(cfg.execute_workers_max_num, 5);
	});

	// Here we spawn 8 validation jobs for the `halt` PVF and share those between 5 workers. The
	// first five jobs should timeout and the workers killed. For the next 3 jobs a new batch of
	// workers should be spun up.
	futures::future::join_all((0u8..=8).map(|_| {
		host.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
		)
	}))
	.await;
}
