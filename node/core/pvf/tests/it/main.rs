// Copyright (C) Parity Technologies (UK) Ltd.
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

use assert_matches::assert_matches;
use parity_scale_codec::Encode as _;
use polkadot_node_core_pvf::{
	start, Config, InvalidCandidate, Metrics, PvfPrepData, ValidationError, ValidationHost,
	JOB_TIMEOUT_WALL_CLOCK_FACTOR,
};
use polkadot_parachain::primitives::{BlockData, ValidationParams, ValidationResult};
use polkadot_primitives::{ExecutorParam, ExecutorParams};
use std::time::Duration;
use tokio::sync::Mutex;

mod adder;
mod worker_common;

const PUPPET_EXE: &str = env!("CARGO_BIN_EXE_puppet_worker");
const TEST_EXECUTION_TIMEOUT: Duration = Duration::from_secs(3);
const TEST_PREPARATION_TIMEOUT: Duration = Duration::from_secs(3);

struct TestHost {
	cache_dir: tempfile::TempDir,
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
		let (host, task) = start(config, Metrics::default());
		let _ = tokio::task::spawn(task);
		Self { cache_dir, host: Mutex::new(host) }
	}

	async fn validate_candidate(
		&self,
		code: &[u8],
		params: ValidationParams,
		executor_params: ExecutorParams,
	) -> Result<ValidationResult, ValidationError> {
		let (result_tx, result_rx) = futures::channel::oneshot::channel();

		let code = sp_maybe_compressed_blob::decompress(code, 16 * 1024 * 1024)
			.expect("Compression works");

		self.host
			.lock()
			.await
			.execute_pvf(
				PvfPrepData::from_code(code.into(), executor_params, TEST_PREPARATION_TIMEOUT),
				TEST_EXECUTION_TIMEOUT,
				params.encode(),
				polkadot_node_core_pvf::Priority::Normal,
				result_tx,
			)
			.await
			.unwrap();
		result_rx.await.unwrap()
	}
}

#[tokio::test]
async fn terminates_on_timeout() {
	let host = TestHost::new();

	let start = std::time::Instant::now();
	let result = host
		.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
			Default::default(),
		)
		.await;

	match result {
		Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout)) => {},
		r => panic!("{:?}", r),
	}

	let duration = std::time::Instant::now().duration_since(start);
	assert!(duration >= TEST_EXECUTION_TIMEOUT);
	assert!(duration < TEST_EXECUTION_TIMEOUT * JOB_TIMEOUT_WALL_CLOCK_FACTOR);
}

#[tokio::test]
async fn ensure_parallel_execution() {
	// Run some jobs that do not complete, thus timing out.
	let host = TestHost::new();
	let execute_pvf_future_1 = host.validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_parent_number: 1,
			relay_parent_storage_root: Default::default(),
		},
		Default::default(),
	);
	let execute_pvf_future_2 = host.validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_parent_number: 1,
			relay_parent_storage_root: Default::default(),
		},
		Default::default(),
	);

	let start = std::time::Instant::now();
	let (res1, res2) = futures::join!(execute_pvf_future_1, execute_pvf_future_2);
	assert_matches!(
		(res1, res2),
		(
			Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout)),
			Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout))
		)
	);

	// Total time should be < 2 x TEST_EXECUTION_TIMEOUT (two workers run in parallel).
	let duration = std::time::Instant::now().duration_since(start);
	let max_duration = 2 * TEST_EXECUTION_TIMEOUT;
	assert!(
		duration < max_duration,
		"Expected duration {}ms to be less than {}ms",
		duration.as_millis(),
		max_duration.as_millis()
	);
}

#[tokio::test]
async fn execute_queue_doesnt_stall_if_workers_died() {
	let host = TestHost::new_with_config(|cfg| {
		cfg.execute_workers_max_num = 5;
	});

	// Here we spawn 8 validation jobs for the `halt` PVF and share those between 5 workers. The
	// first five jobs should timeout and the workers killed. For the next 3 jobs a new batch of
	// workers should be spun up.
	let start = std::time::Instant::now();
	futures::future::join_all((0u8..=8).map(|_| {
		host.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
			Default::default(),
		)
	}))
	.await;

	// Total time should be >= 2 x TEST_EXECUTION_TIMEOUT (two separate sets of workers that should
	// both timeout).
	let duration = std::time::Instant::now().duration_since(start);
	let max_duration = 2 * TEST_EXECUTION_TIMEOUT;
	assert!(
		duration >= max_duration,
		"Expected duration {}ms to be greater than or equal to {}ms",
		duration.as_millis(),
		max_duration.as_millis()
	);
}

#[tokio::test]
async fn execute_queue_doesnt_stall_with_varying_executor_params() {
	let host = TestHost::new_with_config(|cfg| {
		cfg.execute_workers_max_num = 2;
	});

	let executor_params_1 = ExecutorParams::default();
	let executor_params_2 = ExecutorParams::from(&[ExecutorParam::StackLogicalMax(1024)][..]);

	// Here we spawn 6 validation jobs for the `halt` PVF and share those between 2 workers. Every
	// 3rd job will have different set of executor parameters. All the workers should be killed
	// and in this case the queue should respawn new workers with needed executor environment
	// without waiting. The jobs will be executed in 3 batches, each running two jobs in parallel,
	// and execution time would be roughly 3 * TEST_EXECUTION_TIMEOUT
	let start = std::time::Instant::now();
	futures::future::join_all((0u8..6).map(|i| {
		host.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
			match i % 3 {
				0 => executor_params_1.clone(),
				_ => executor_params_2.clone(),
			},
		)
	}))
	.await;

	let duration = std::time::Instant::now().duration_since(start);
	let min_duration = 3 * TEST_EXECUTION_TIMEOUT;
	let max_duration = 4 * TEST_EXECUTION_TIMEOUT;
	assert!(
		duration >= min_duration,
		"Expected duration {}ms to be greater than or equal to {}ms",
		duration.as_millis(),
		min_duration.as_millis()
	);
	assert!(
		duration <= max_duration,
		"Expected duration {}ms to be less than or equal to {}ms",
		duration.as_millis(),
		max_duration.as_millis()
	);
}

// Test that deleting a prepared artifact does not lead to a dispute when we try to execute it.
#[tokio::test]
async fn deleting_prepared_artifact_does_not_dispute() {
	let host = TestHost::new();
	let cache_dir = host.cache_dir.path().clone();

	let result = host
		.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
			Default::default(),
		)
		.await;

	match result {
		Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout)) => {},
		r => panic!("{:?}", r),
	}

	// Delete the prepared artifact.
	{
		// Get the artifact path (asserting it exists).
		let mut cache_dir: Vec<_> = std::fs::read_dir(cache_dir).unwrap().collect();
		assert_eq!(cache_dir.len(), 1);
		let artifact_path = cache_dir.pop().unwrap().unwrap();

		// Delete the artifact.
		std::fs::remove_file(artifact_path.path()).unwrap();
	}

	// Try to validate again, artifact should get recreated.
	let result = host
		.validate_candidate(
			halt::wasm_binary_unwrap(),
			ValidationParams {
				block_data: BlockData(Vec::new()),
				parent_head: Default::default(),
				relay_parent_number: 1,
				relay_parent_storage_root: Default::default(),
			},
			Default::default(),
		)
		.await;

	match result {
		Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout)) => {},
		r => panic!("{:?}", r),
	}
}
