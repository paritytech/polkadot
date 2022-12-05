// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

use super::*;
use ::test_helpers::{dummy_hash, make_valid_candidate_descriptor};
use assert_matches::assert_matches;
use futures::executor;
use polkadot_node_core_pvf::PrepareError;
use polkadot_node_subsystem::messages::AllMessages;
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::reexports::SubsystemContext;
use polkadot_primitives::v2::{HeadData, Id as ParaId, UpwardMessage};
use sp_core::testing::TaskExecutor;
use sp_keyring::Sr25519Keyring;

#[test]
fn correctly_checks_included_assumption() {
	let validation_data: PersistedValidationData = Default::default();
	let validation_code: ValidationCode = vec![1, 2, 3].into();

	let persisted_validation_data_hash = validation_data.hash();
	let relay_parent = [2; 32].into();
	let para_id = ParaId::from(5_u32);

	let descriptor = make_valid_candidate_descriptor(
		para_id,
		relay_parent,
		persisted_validation_data_hash,
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = check_assumption_validation_data(
		ctx.sender(),
		&descriptor,
		OccupiedCoreAssumption::Included,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::PersistedValidationData(
					p,
					OccupiedCoreAssumption::Included,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(Some(validation_data.clone())));
			}
		);

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::ValidationCode(p, OccupiedCoreAssumption::Included, tx)
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(Some(validation_code.clone())));
			}
		);

		assert_matches!(check_result.await, AssumptionCheckOutcome::Matches(o, v) => {
			assert_eq!(o, validation_data);
			assert_eq!(v, validation_code);
		});
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn correctly_checks_timed_out_assumption() {
	let validation_data: PersistedValidationData = Default::default();
	let validation_code: ValidationCode = vec![1, 2, 3].into();

	let persisted_validation_data_hash = validation_data.hash();
	let relay_parent = [2; 32].into();
	let para_id = ParaId::from(5_u32);

	let descriptor = make_valid_candidate_descriptor(
		para_id,
		relay_parent,
		persisted_validation_data_hash,
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = check_assumption_validation_data(
		ctx.sender(),
		&descriptor,
		OccupiedCoreAssumption::TimedOut,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::PersistedValidationData(
					p,
					OccupiedCoreAssumption::TimedOut,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(Some(validation_data.clone())));
			}
		);

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::ValidationCode(p, OccupiedCoreAssumption::TimedOut, tx)
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(Some(validation_code.clone())));
			}
		);

		assert_matches!(check_result.await, AssumptionCheckOutcome::Matches(o, v) => {
			assert_eq!(o, validation_data);
			assert_eq!(v, validation_code);
		});
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn check_is_bad_request_if_no_validation_data() {
	let validation_data: PersistedValidationData = Default::default();
	let persisted_validation_data_hash = validation_data.hash();
	let relay_parent = [2; 32].into();
	let para_id = ParaId::from(5_u32);

	let descriptor = make_valid_candidate_descriptor(
		para_id,
		relay_parent,
		persisted_validation_data_hash,
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = check_assumption_validation_data(
		ctx.sender(),
		&descriptor,
		OccupiedCoreAssumption::Included,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::PersistedValidationData(
					p,
					OccupiedCoreAssumption::Included,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(None));
			}
		);

		assert_matches!(check_result.await, AssumptionCheckOutcome::BadRequest);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn check_is_bad_request_if_no_validation_code() {
	let validation_data: PersistedValidationData = Default::default();
	let persisted_validation_data_hash = validation_data.hash();
	let relay_parent = [2; 32].into();
	let para_id = ParaId::from(5_u32);

	let descriptor = make_valid_candidate_descriptor(
		para_id,
		relay_parent,
		persisted_validation_data_hash,
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = check_assumption_validation_data(
		ctx.sender(),
		&descriptor,
		OccupiedCoreAssumption::TimedOut,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::PersistedValidationData(
					p,
					OccupiedCoreAssumption::TimedOut,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(Some(validation_data.clone())));
			}
		);

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::ValidationCode(p, OccupiedCoreAssumption::TimedOut, tx)
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(None));
			}
		);

		assert_matches!(check_result.await, AssumptionCheckOutcome::BadRequest);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn check_does_not_match() {
	let validation_data: PersistedValidationData = Default::default();
	let relay_parent = Hash::repeat_byte(0x02);
	let para_id = ParaId::from(5_u32);

	let descriptor = make_valid_candidate_descriptor(
		para_id,
		relay_parent,
		Hash::from([3; 32]),
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = check_assumption_validation_data(
		ctx.sender(),
		&descriptor,
		OccupiedCoreAssumption::Included,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::PersistedValidationData(
					p,
					OccupiedCoreAssumption::Included,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(Some(validation_data.clone())));
			}
		);

		assert_matches!(check_result.await, AssumptionCheckOutcome::DoesNotMatch);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

struct MockValidateCandidateBackend {
	result_list: Vec<Result<WasmValidationResult, ValidationError>>,
	num_times_called: usize,
}

impl MockValidateCandidateBackend {
	fn with_hardcoded_result(result: Result<WasmValidationResult, ValidationError>) -> Self {
		Self { result_list: vec![result], num_times_called: 0 }
	}

	fn with_hardcoded_result_list(
		result_list: Vec<Result<WasmValidationResult, ValidationError>>,
	) -> Self {
		Self { result_list, num_times_called: 0 }
	}
}

#[async_trait]
impl ValidationBackend for MockValidateCandidateBackend {
	async fn validate_candidate(
		&mut self,
		_pvf: Pvf,
		_timeout: Duration,
		_encoded_params: Vec<u8>,
	) -> Result<WasmValidationResult, ValidationError> {
		// This is expected to panic if called more times than expected, indicating an error in the
		// test.
		let result = self.result_list[self.num_times_called].clone();
		self.num_times_called += 1;

		result
	}

	async fn precheck_pvf(&mut self, _pvf: Pvf) -> Result<Duration, PrepareError> {
		unreachable!()
	}
}

#[test]
fn candidate_validation_ok_is_ok() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);
	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		head_data.hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		&validation_code.hash(),
	);
	assert!(check.is_ok());

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: Some(vec![2, 2, 2].into()),
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let commitments = CandidateCommitments {
		head_data: validation_result.head_data.clone(),
		upward_messages: validation_result.upward_messages.clone(),
		horizontal_messages: validation_result.horizontal_messages.clone(),
		new_validation_code: validation_result.new_validation_code.clone(),
		processed_downward_messages: validation_result.processed_downward_messages,
		hrmp_watermark: validation_result.hrmp_watermark,
	};

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: commitments.hash() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data.clone(),
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Valid(outputs, used_validation_data) => {
		assert_eq!(outputs.head_data, HeadData(vec![1, 1, 1]));
		assert_eq!(outputs.upward_messages, Vec::<UpwardMessage>::new());
		assert_eq!(outputs.horizontal_messages, Vec::new());
		assert_eq!(outputs.new_validation_code, Some(vec![2, 2, 2].into()));
		assert_eq!(outputs.hrmp_watermark, 0);
		assert_eq!(used_validation_data, validation_data);
	});
}

#[test]
fn candidate_validation_bad_return_is_invalid() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		&validation_code.hash(),
	);
	assert!(check.is_ok());

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: Hash::zero() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Err(
			ValidationError::InvalidCandidate(WasmInvalidCandidate::HardTimeout),
		)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Invalid(InvalidCandidate::Timeout));
}

#[test]
fn candidate_validation_one_ambiguous_error_is_valid() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);
	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		head_data.hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		&validation_code.hash(),
	);
	assert!(check.is_ok());

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: Some(vec![2, 2, 2].into()),
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let commitments = CandidateCommitments {
		head_data: validation_result.head_data.clone(),
		upward_messages: validation_result.upward_messages.clone(),
		horizontal_messages: validation_result.horizontal_messages.clone(),
		new_validation_code: validation_result.new_validation_code.clone(),
		processed_downward_messages: validation_result.processed_downward_messages,
		hrmp_watermark: validation_result.hrmp_watermark,
	};

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: commitments.hash() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result_list(vec![
			Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::AmbiguousWorkerDeath)),
			Ok(validation_result),
		]),
		validation_data.clone(),
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Valid(outputs, used_validation_data) => {
		assert_eq!(outputs.head_data, HeadData(vec![1, 1, 1]));
		assert_eq!(outputs.upward_messages, Vec::<UpwardMessage>::new());
		assert_eq!(outputs.horizontal_messages, Vec::new());
		assert_eq!(outputs.new_validation_code, Some(vec![2, 2, 2].into()));
		assert_eq!(outputs.hrmp_watermark, 0);
		assert_eq!(used_validation_data, validation_data);
	});
}

#[test]
fn candidate_validation_multiple_ambiguous_errors_is_invalid() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		&validation_code.hash(),
	);
	assert!(check.is_ok());

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: Hash::zero() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result_list(vec![
			Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::AmbiguousWorkerDeath)),
			Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::AmbiguousWorkerDeath)),
		]),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Invalid(InvalidCandidate::ExecutionError(_)));
}

#[test]
fn candidate_validation_timeout_is_internal_error() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		&validation_code.hash(),
	);
	assert!(check.is_ok());

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: Hash::zero() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Err(
			ValidationError::InvalidCandidate(WasmInvalidCandidate::HardTimeout),
		)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::Timeout)));
}

#[test]
fn candidate_validation_commitment_hash_mismatch_is_invalid() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };
	let pov = PoV { block_data: BlockData(vec![0xff; 32]) };
	let validation_code = ValidationCode(vec![0xff; 16]);
	let head_data = HeadData(vec![1, 1, 1]);

	let candidate_receipt = CandidateReceipt {
		descriptor: make_valid_candidate_descriptor(
			ParaId::from(1_u32),
			validation_data.parent_head.hash(),
			validation_data.hash(),
			pov.hash(),
			validation_code.hash(),
			head_data.hash(),
			dummy_hash(),
			Sr25519Keyring::Alice,
		),
		commitments_hash: Hash::zero(),
	};

	// This will result in different commitments for this candidate.
	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 12345,
	};

	let result = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	// Ensure `post validation` check on the commitments hash works as expected.
	assert_matches!(result, ValidationResult::Invalid(InvalidCandidate::CommitmentsHashMismatch));
}

#[test]
fn candidate_validation_code_mismatch_is_invalid() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		ValidationCode(vec![1; 16]).hash(),
		dummy_hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		&validation_code.hash(),
	);
	assert_matches!(check, Err(InvalidCandidate::CodeHashMismatch));

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: Hash::zero() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Err(
			ValidationError::InvalidCandidate(WasmInvalidCandidate::HardTimeout),
		)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Invalid(InvalidCandidate::CodeHashMismatch));
}

#[test]
fn compressed_code_works() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };
	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);

	let raw_code = vec![2u8; 16];
	let validation_code = sp_maybe_compressed_blob::compress(&raw_code, VALIDATION_CODE_BOMB_LIMIT)
		.map(ValidationCode)
		.unwrap();

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		head_data.hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let commitments = CandidateCommitments {
		head_data: validation_result.head_data.clone(),
		upward_messages: validation_result.upward_messages.clone(),
		horizontal_messages: validation_result.horizontal_messages.clone(),
		new_validation_code: validation_result.new_validation_code.clone(),
		processed_downward_messages: validation_result.processed_downward_messages,
		hrmp_watermark: validation_result.hrmp_watermark,
	};

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: commitments.hash() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Valid(_, _)));
}

#[test]
fn code_decompression_failure_is_invalid() {
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };
	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);

	let raw_code = vec![2u8; VALIDATION_CODE_BOMB_LIMIT + 1];
	let validation_code =
		sp_maybe_compressed_blob::compress(&raw_code, VALIDATION_CODE_BOMB_LIMIT + 1)
			.map(ValidationCode)
			.unwrap();

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		head_data.hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: Hash::zero() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::CodeDecompressionFailure)));
}

#[test]
fn pov_decompression_failure_is_invalid() {
	let validation_data =
		PersistedValidationData { max_pov_size: POV_BOMB_LIMIT as u32, ..Default::default() };
	let head_data = HeadData(vec![1, 1, 1]);

	let raw_block_data = vec![2u8; POV_BOMB_LIMIT + 1];
	let pov = sp_maybe_compressed_blob::compress(&raw_block_data, POV_BOMB_LIMIT + 1)
		.map(|raw| PoV { block_data: BlockData(raw) })
		.unwrap();

	let validation_code = ValidationCode(vec![2; 16]);

	let descriptor = make_valid_candidate_descriptor(
		ParaId::from(1_u32),
		dummy_hash(),
		validation_data.hash(),
		pov.hash(),
		validation_code.hash(),
		head_data.hash(),
		dummy_hash(),
		Sr25519Keyring::Alice,
	);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: Hash::zero() };

	let v = executor::block_on(validate_candidate_exhaustive(
		MockValidateCandidateBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		validation_code,
		candidate_receipt,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::PoVDecompressionFailure)));
}

struct MockPreCheckBackend {
	result: Result<Duration, PrepareError>,
}

impl MockPreCheckBackend {
	fn with_hardcoded_result(result: Result<Duration, PrepareError>) -> Self {
		Self { result }
	}
}

#[async_trait]
impl ValidationBackend for MockPreCheckBackend {
	async fn validate_candidate(
		&mut self,
		_pvf: Pvf,
		_timeout: Duration,
		_encoded_params: Vec<u8>,
	) -> Result<WasmValidationResult, ValidationError> {
		unreachable!()
	}

	async fn precheck_pvf(&mut self, _pvf: Pvf) -> Result<Duration, PrepareError> {
		self.result.clone()
	}
}

#[test]
fn precheck_works() {
	let relay_parent = [3; 32].into();
	let validation_code = ValidationCode(vec![3; 16]);
	let validation_code_hash = validation_code.hash();

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = precheck_pvf(
		ctx.sender(),
		MockPreCheckBackend::with_hardcoded_result(Ok(Duration::default())),
		relay_parent,
		validation_code_hash,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::ValidationCodeByHash(
					vch,
					tx
				),
			)) => {
				assert_eq!(vch, validation_code_hash);
				assert_eq!(rp, relay_parent);

				let _ = tx.send(Ok(Some(validation_code.clone())));
			}
		);
		assert_matches!(check_result.await, PreCheckOutcome::Valid);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn precheck_invalid_pvf_blob_compression() {
	let relay_parent = [3; 32].into();

	let raw_code = vec![2u8; VALIDATION_CODE_BOMB_LIMIT + 1];
	let validation_code =
		sp_maybe_compressed_blob::compress(&raw_code, VALIDATION_CODE_BOMB_LIMIT + 1)
			.map(ValidationCode)
			.unwrap();
	let validation_code_hash = validation_code.hash();

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = precheck_pvf(
		ctx.sender(),
		MockPreCheckBackend::with_hardcoded_result(Ok(Duration::default())),
		relay_parent,
		validation_code_hash,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::ValidationCodeByHash(
					vch,
					tx
				),
			)) => {
				assert_eq!(vch, validation_code_hash);
				assert_eq!(rp, relay_parent);

				let _ = tx.send(Ok(Some(validation_code.clone())));
			}
		);
		assert_matches!(check_result.await, PreCheckOutcome::Invalid);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn precheck_properly_classifies_outcomes() {
	let inner = |prepare_result, precheck_outcome| {
		let relay_parent = [3; 32].into();
		let validation_code = ValidationCode(vec![3; 16]);
		let validation_code_hash = validation_code.hash();

		let pool = TaskExecutor::new();
		let (mut ctx, mut ctx_handle) =
			test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

		let (check_fut, check_result) = precheck_pvf(
			ctx.sender(),
			MockPreCheckBackend::with_hardcoded_result(prepare_result),
			relay_parent,
			validation_code_hash,
		)
		.remote_handle();

		let test_fut = async move {
			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::ValidationCodeByHash(
						vch,
						tx
					),
				)) => {
					assert_eq!(vch, validation_code_hash);
					assert_eq!(rp, relay_parent);

					let _ = tx.send(Ok(Some(validation_code.clone())));
				}
			);
			assert_eq!(check_result.await, precheck_outcome);
		};

		let test_fut = future::join(test_fut, check_fut);
		executor::block_on(test_fut);
	};

	inner(Err(PrepareError::Prevalidation("foo".to_owned())), PreCheckOutcome::Invalid);
	inner(Err(PrepareError::Preparation("bar".to_owned())), PreCheckOutcome::Invalid);
	inner(Err(PrepareError::Panic("baz".to_owned())), PreCheckOutcome::Invalid);

	inner(Err(PrepareError::TimedOut), PreCheckOutcome::Failed);
	inner(Err(PrepareError::DidNotMakeIt), PreCheckOutcome::Failed);
}
