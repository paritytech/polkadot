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
use assert_matches::assert_matches;
use futures::executor;
use polkadot_node_subsystem::messages::AllMessages;
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::reexports::SubsystemContext;
use polkadot_primitives::v1::{HeadData, UpwardMessage};
use sp_core::testing::TaskExecutor;
use sp_keyring::Sr25519Keyring;

fn collator_sign(descriptor: &mut CandidateDescriptor, collator: Sr25519Keyring) {
	descriptor.collator = collator.public().into();
	let payload = polkadot_primitives::v1::collator_signature_payload(
		&descriptor.relay_parent,
		&descriptor.para_id,
		&descriptor.persisted_validation_data_hash,
		&descriptor.pov_hash,
		&descriptor.validation_code_hash,
	);

	descriptor.signature = collator.sign(&payload[..]).into();
	assert!(descriptor.check_collator_signature().is_ok());
}

#[test]
fn correctly_finds_assumed_validation_data() {
	let validation_data: PersistedValidationData = Default::default();

	let persisted_validation_data_hash = validation_data.hash();
	let relay_parent = [2; 32].into();
	let para_id = 5.into();

	let mut candidate = CandidateDescriptor::default();
	candidate.relay_parent = relay_parent;
	candidate.persisted_validation_data_hash = persisted_validation_data_hash;
	candidate.para_id = para_id;

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) =
		request_assumed_validation_data(ctx.sender(), &candidate).remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::AssumedValidationData(
					p,
					hash,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);
				assert_eq!(hash, persisted_validation_data_hash);

				let _ = tx.send(Ok(Some((validation_data.clone(), Default::default()))));
			}
		);

		assert_matches!(check_result.await, Ok(Some((d, _))) => {
			assert_eq!(d, validation_data);
		});
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn check_is_none_if_no_validation_data() {
	let validation_data: PersistedValidationData = Default::default();
	let persisted_validation_data_hash = validation_data.hash();
	let relay_parent = [2; 32].into();
	let para_id = 5.into();

	let mut candidate = CandidateDescriptor::default();
	candidate.relay_parent = relay_parent;
	candidate.persisted_validation_data_hash = persisted_validation_data_hash;
	candidate.para_id = para_id;

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) =
		request_assumed_validation_data(ctx.sender(), &candidate).remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::AssumedValidationData(
					p,
					_,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Ok(None));
			}
		);

		assert_matches!(check_result.await, Ok(None));
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn check_runtime_api_error() {
	let relay_parent = [2; 32].into();
	let para_id = 5.into();

	let mut candidate = CandidateDescriptor::default();
	candidate.relay_parent = relay_parent;
	candidate.persisted_validation_data_hash = [3; 32].into();
	candidate.para_id = para_id;

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) =
		request_assumed_validation_data(ctx.sender(), &candidate).remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				rp,
				RuntimeApiRequest::AssumedValidationData(
					p,
					_,
					tx
				),
			)) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(p, para_id);

				let _ = tx.send(Err(RuntimeApiError::from(String::new())));
			}
		);

		assert_matches!(check_result.await, Err(RuntimeRequestFailed));
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

struct MockValidatorBackend {
	result: Result<WasmValidationResult, ValidationError>,
}

impl MockValidatorBackend {
	fn with_hardcoded_result(result: Result<WasmValidationResult, ValidationError>) -> Self {
		Self { result }
	}
}

#[async_trait]
impl ValidationBackend for MockValidatorBackend {
	async fn validate_candidate(
		&mut self,
		raw_validation_code: Pvf,
		_timeout: Duration,
		_params: ValidationParams,
	) -> Result<WasmValidationResult, ValidationError> {
		match raw_validation_code {
			Pvf::Preimage(_) => self.result.clone(),
			Pvf::Hash(_) => Err(ValidationError::ArtifactNotFound),
		}
	}
}

#[test]
fn check_runtime_validation_code_request() {
	let metrics = Default::default();
	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);
	let validation_code_hash = validation_code.hash();

	let head_data = HeadData(vec![1, 1, 1]);
	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code_hash;
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: Some(vec![2, 2, 2].into()),
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let (check_fut, check_result) = validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data.clone(),
		None,
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&metrics,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::ValidationCodeByHash(
					validation_code_hash,
					tx
				),
			)) => {
				assert_eq!(validation_code_hash, validation_code.hash());

				let _ = tx.send(Ok(Some(validation_code)));
			}
		);

		assert_matches!(check_result.await, Ok(ValidationResult::Valid(_, _)));
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn candidate_validation_ok_is_ok() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);
	let validation_code = ValidationCode(vec![2; 16]);

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		Some(&validation_code.hash()),
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

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data.clone(),
		Some(validation_code),
		descriptor,
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
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		Some(&validation_code.hash()),
	);
	assert!(check.is_ok());

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Err(ValidationError::InvalidCandidate(
			WasmInvalidCandidate::AmbigiousWorkerDeath,
		))),
		validation_data,
		Some(validation_code),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Invalid(InvalidCandidate::ExecutionError(_)));
}

#[test]
fn candidate_validation_timeout_is_internal_error() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		Some(&validation_code.hash()),
	);
	assert!(check.is_ok());

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Err(ValidationError::InvalidCandidate(
			WasmInvalidCandidate::HardTimeout,
		))),
		validation_data,
		Some(validation_code),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::Timeout)));
}

#[test]
fn candidate_validation_code_mismatch_is_invalid() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let validation_code = ValidationCode(vec![2; 16]);

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.validation_code_hash = ValidationCode(vec![1; 16]).hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let check = perform_basic_checks(
		&descriptor,
		validation_data.max_pov_size,
		&pov,
		Some(&validation_code.hash()),
	);
	assert_matches!(check, Err(InvalidCandidate::CodeHashMismatch));

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Err(ValidationError::InvalidCandidate(
			WasmInvalidCandidate::HardTimeout,
		))),
		validation_data,
		Some(validation_code),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	))
	.unwrap();

	assert_matches!(v, ValidationResult::Invalid(InvalidCandidate::CodeHashMismatch));
}

#[test]
fn compressed_code_works() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };
	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);

	let raw_code = vec![2u8; 16];
	let validation_code = sp_maybe_compressed_blob::compress(&raw_code, VALIDATION_CODE_BOMB_LIMIT)
		.map(ValidationCode)
		.unwrap();

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		Some(validation_code),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Valid(_, _)));
}

#[test]
fn code_decompression_failure_is_invalid() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };
	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);

	let raw_code = vec![2u8; VALIDATION_CODE_BOMB_LIMIT + 1];
	let validation_code =
		sp_maybe_compressed_blob::compress(&raw_code, VALIDATION_CODE_BOMB_LIMIT + 1)
			.map(ValidationCode)
			.unwrap();

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		Some(validation_code),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::CodeDecompressionFailure)));
}

#[test]
fn pov_decompression_failure_is_invalid() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut _ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data =
		PersistedValidationData { max_pov_size: POV_BOMB_LIMIT as u32, ..Default::default() };
	let head_data = HeadData(vec![1, 1, 1]);

	let raw_block_data = vec![2u8; POV_BOMB_LIMIT + 1];
	let pov = sp_maybe_compressed_blob::compress(&raw_block_data, POV_BOMB_LIMIT + 1)
		.map(|raw| PoV { block_data: BlockData(raw) })
		.unwrap();

	let validation_code = ValidationCode(vec![2; 16]);

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: None,
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let v = executor::block_on(validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		Some(validation_code),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&Default::default(),
	));

	assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::PoVDecompressionFailure)));
}

#[test]
fn requests_code_from_api_if_not_found() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);
	let validation_code = ValidationCode(vec![2; 16]);
	let validation_code_hash = validation_code.hash();

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: Some(vec![2, 2, 2].into()),
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let metrics = Default::default();

	let (check_fut, check_result) = validate_candidate_exhaustive(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		validation_data,
		None,
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&metrics,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_rp,
				RuntimeApiRequest::ValidationCodeByHash(
					code_hash,
					tx
				),
			)) => {
				assert_eq!(code_hash, validation_code_hash);

				let _ = tx.send(Ok(Some(validation_code)));
			}
		);

		assert_matches!(check_result.await, Ok(ValidationResult::Valid(_, _)));
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}

#[test]
fn code_hash_mismatch_error() {
	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<AllMessages, _>(pool.clone());

	let validation_data = PersistedValidationData { max_pov_size: 1024, ..Default::default() };

	let pov = PoV { block_data: BlockData(vec![1; 32]) };
	let head_data = HeadData(vec![1, 1, 1]);
	let validation_code = ValidationCode(vec![2; 16]);

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov.hash();
	descriptor.para_head = head_data.hash();
	descriptor.validation_code_hash = validation_code.hash();
	collator_sign(&mut descriptor, Sr25519Keyring::Alice);

	let validation_result = WasmValidationResult {
		head_data,
		new_validation_code: Some(vec![2, 2, 2].into()),
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		processed_downward_messages: 0,
		hrmp_watermark: 0,
	};

	let metrics = Default::default();

	let (check_fut, check_result) = validate_from_chain_state(
		ctx.sender(),
		MockValidatorBackend::with_hardcoded_result(Ok(validation_result)),
		descriptor,
		Arc::new(pov),
		Duration::from_secs(0),
		&metrics,
	)
	.remote_handle();

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_rp,
				RuntimeApiRequest::AssumedValidationData(
					_p,
					_hash,
					tx
				),
			)) => {
				let code_hash = ValidationCodeHash::from(Hash::zero());
				let _ = tx.send(Ok(Some((validation_data, code_hash))));
			}
		);

		assert_matches!(
			check_result.await,
			Ok(ValidationResult::Invalid(InvalidCandidate::CodeHashMismatch))
		);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}
