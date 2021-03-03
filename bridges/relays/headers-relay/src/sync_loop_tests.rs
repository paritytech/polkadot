// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

#![cfg(test)]

use crate::sync_loop::{run, SourceClient, TargetClient};
use crate::sync_types::{HeadersSyncPipeline, QueuedHeader, SourceHeader, SubmittedHeaders};

use async_trait::async_trait;
use backoff::backoff::Backoff;
use futures::{future::FutureExt, stream::StreamExt};
use parking_lot::Mutex;
use relay_utils::{
	process_future_result, relay_loop::Client as RelayClient, retry_backoff, HeaderId, MaybeConnectionError,
};
use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
	time::Duration,
};

pub type TestNumber = u64;
pub type TestHash = u64;
pub type TestHeaderId = HeaderId<TestHash, TestNumber>;
pub type TestExtra = u64;
pub type TestCompletion = u64;
pub type TestQueuedHeader = QueuedHeader<TestHeadersSyncPipeline>;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct TestHeader {
	pub hash: TestHash,
	pub number: TestNumber,
	pub parent_hash: TestHash,
}

impl SourceHeader<TestHash, TestNumber> for TestHeader {
	fn id(&self) -> TestHeaderId {
		HeaderId(self.number, self.hash)
	}

	fn parent_id(&self) -> TestHeaderId {
		HeaderId(self.number - 1, self.parent_hash)
	}
}

#[derive(Debug, Clone)]
struct TestError(bool);

impl MaybeConnectionError for TestError {
	fn is_connection_error(&self) -> bool {
		self.0
	}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TestHeadersSyncPipeline;

impl HeadersSyncPipeline for TestHeadersSyncPipeline {
	const SOURCE_NAME: &'static str = "Source";
	const TARGET_NAME: &'static str = "Target";

	type Hash = TestHash;
	type Number = TestNumber;
	type Header = TestHeader;
	type Extra = TestExtra;
	type Completion = TestCompletion;

	fn estimate_size(_: &TestQueuedHeader) -> usize {
		0
	}
}

enum SourceMethod {
	BestBlockNumber,
	HeaderByHash(TestHash),
	HeaderByNumber(TestNumber),
	HeaderCompletion(TestHeaderId),
	HeaderExtra(TestHeaderId, TestQueuedHeader),
}

#[derive(Clone)]
struct Source {
	data: Arc<Mutex<SourceData>>,
	on_method_call: Arc<dyn Fn(SourceMethod, &mut SourceData) + Send + Sync>,
}

struct SourceData {
	best_block_number: Result<TestNumber, TestError>,
	header_by_hash: HashMap<TestHash, TestHeader>,
	header_by_number: HashMap<TestNumber, TestHeader>,
	provides_completion: bool,
	provides_extra: bool,
}

impl Source {
	pub fn new(
		best_block_id: TestHeaderId,
		headers: Vec<(bool, TestHeader)>,
		on_method_call: impl Fn(SourceMethod, &mut SourceData) + Send + Sync + 'static,
	) -> Self {
		Source {
			data: Arc::new(Mutex::new(SourceData {
				best_block_number: Ok(best_block_id.0),
				header_by_hash: headers
					.iter()
					.map(|(_, header)| (header.hash, header.clone()))
					.collect(),
				header_by_number: headers
					.iter()
					.filter_map(|(is_canonical, header)| {
						if *is_canonical {
							Some((header.hash, header.clone()))
						} else {
							None
						}
					})
					.collect(),
				provides_completion: true,
				provides_extra: true,
			})),
			on_method_call: Arc::new(on_method_call),
		}
	}
}

#[async_trait]
impl RelayClient for Source {
	type Error = TestError;

	async fn reconnect(&mut self) -> Result<(), TestError> {
		unimplemented!()
	}
}

#[async_trait]
impl SourceClient<TestHeadersSyncPipeline> for Source {
	async fn best_block_number(&self) -> Result<TestNumber, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(SourceMethod::BestBlockNumber, &mut *data);
		data.best_block_number.clone()
	}

	async fn header_by_hash(&self, hash: TestHash) -> Result<TestHeader, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(SourceMethod::HeaderByHash(hash), &mut *data);
		data.header_by_hash.get(&hash).cloned().ok_or(TestError(false))
	}

	async fn header_by_number(&self, number: TestNumber) -> Result<TestHeader, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(SourceMethod::HeaderByNumber(number), &mut *data);
		data.header_by_number.get(&number).cloned().ok_or(TestError(false))
	}

	async fn header_completion(&self, id: TestHeaderId) -> Result<(TestHeaderId, Option<TestCompletion>), TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(SourceMethod::HeaderCompletion(id), &mut *data);
		if data.provides_completion {
			Ok((id, Some(test_completion(id))))
		} else {
			Ok((id, None))
		}
	}

	async fn header_extra(
		&self,
		id: TestHeaderId,
		header: TestQueuedHeader,
	) -> Result<(TestHeaderId, TestExtra), TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(SourceMethod::HeaderExtra(id, header), &mut *data);
		if data.provides_extra {
			Ok((id, test_extra(id)))
		} else {
			Err(TestError(false))
		}
	}
}

enum TargetMethod {
	BestHeaderId,
	IsKnownHeader(TestHeaderId),
	SubmitHeaders(Vec<TestQueuedHeader>),
	IncompleteHeadersIds,
	CompleteHeader(TestHeaderId, TestCompletion),
	RequiresExtra(TestQueuedHeader),
}

#[derive(Clone)]
struct Target {
	data: Arc<Mutex<TargetData>>,
	on_method_call: Arc<dyn Fn(TargetMethod, &mut TargetData) + Send + Sync>,
}

struct TargetData {
	best_header_id: Result<TestHeaderId, TestError>,
	is_known_header_by_hash: HashMap<TestHash, bool>,
	submitted_headers: HashMap<TestHash, TestQueuedHeader>,
	submit_headers_result: Option<SubmittedHeaders<TestHeaderId, TestError>>,
	completed_headers: HashMap<TestHash, TestCompletion>,
	requires_completion: bool,
	requires_extra: bool,
}

impl Target {
	pub fn new(
		best_header_id: TestHeaderId,
		headers: Vec<TestHeaderId>,
		on_method_call: impl Fn(TargetMethod, &mut TargetData) + Send + Sync + 'static,
	) -> Self {
		Target {
			data: Arc::new(Mutex::new(TargetData {
				best_header_id: Ok(best_header_id),
				is_known_header_by_hash: headers.iter().map(|header| (header.1, true)).collect(),
				submitted_headers: HashMap::new(),
				submit_headers_result: None,
				completed_headers: HashMap::new(),
				requires_completion: false,
				requires_extra: false,
			})),
			on_method_call: Arc::new(on_method_call),
		}
	}
}

#[async_trait]
impl RelayClient for Target {
	type Error = TestError;

	async fn reconnect(&mut self) -> Result<(), TestError> {
		unimplemented!()
	}
}

#[async_trait]
impl TargetClient<TestHeadersSyncPipeline> for Target {
	async fn best_header_id(&self) -> Result<TestHeaderId, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(TargetMethod::BestHeaderId, &mut *data);
		data.best_header_id.clone()
	}

	async fn is_known_header(&self, id: TestHeaderId) -> Result<(TestHeaderId, bool), TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(TargetMethod::IsKnownHeader(id), &mut *data);
		data.is_known_header_by_hash
			.get(&id.1)
			.cloned()
			.map(|is_known_header| Ok((id, is_known_header)))
			.unwrap_or(Ok((id, false)))
	}

	async fn submit_headers(&self, headers: Vec<TestQueuedHeader>) -> SubmittedHeaders<TestHeaderId, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(TargetMethod::SubmitHeaders(headers.clone()), &mut *data);
		data.submitted_headers
			.extend(headers.iter().map(|header| (header.id().1, header.clone())));
		data.submit_headers_result.take().expect("test must accept headers")
	}

	async fn incomplete_headers_ids(&self) -> Result<HashSet<TestHeaderId>, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(TargetMethod::IncompleteHeadersIds, &mut *data);
		if data.requires_completion {
			Ok(data
				.submitted_headers
				.iter()
				.filter(|(hash, _)| !data.completed_headers.contains_key(hash))
				.map(|(_, header)| header.id())
				.collect())
		} else {
			Ok(HashSet::new())
		}
	}

	async fn complete_header(&self, id: TestHeaderId, completion: TestCompletion) -> Result<TestHeaderId, TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(TargetMethod::CompleteHeader(id, completion), &mut *data);
		data.completed_headers.insert(id.1, completion);
		Ok(id)
	}

	async fn requires_extra(&self, header: TestQueuedHeader) -> Result<(TestHeaderId, bool), TestError> {
		let mut data = self.data.lock();
		(self.on_method_call)(TargetMethod::RequiresExtra(header.clone()), &mut *data);
		if data.requires_extra {
			Ok((header.id(), true))
		} else {
			Ok((header.id(), false))
		}
	}
}

fn test_tick() -> Duration {
	// in ideal world that should have been Duration::from_millis(0), because we do not want
	// to sleep in tests at all, but that could lead to `select! {}` always waking on tick
	// => not doing actual job
	Duration::from_millis(10)
}

fn test_id(number: TestNumber) -> TestHeaderId {
	HeaderId(number, number)
}

fn test_header(number: TestNumber) -> TestHeader {
	let id = test_id(number);
	TestHeader {
		hash: id.1,
		number: id.0,
		parent_hash: if number == 0 {
			TestHash::default()
		} else {
			test_id(number - 1).1
		},
	}
}

fn test_forked_id(number: TestNumber, forked_from: TestNumber) -> TestHeaderId {
	const FORK_OFFSET: TestNumber = 1000;

	if number == forked_from {
		HeaderId(number, number)
	} else {
		HeaderId(number, FORK_OFFSET + number)
	}
}

fn test_forked_header(number: TestNumber, forked_from: TestNumber) -> TestHeader {
	let id = test_forked_id(number, forked_from);
	TestHeader {
		hash: id.1,
		number: id.0,
		parent_hash: if number == 0 {
			TestHash::default()
		} else {
			test_forked_id(number - 1, forked_from).1
		},
	}
}

fn test_completion(id: TestHeaderId) -> TestCompletion {
	id.0
}

fn test_extra(id: TestHeaderId) -> TestExtra {
	id.0
}

fn source_reject_completion(method: &SourceMethod) {
	if let SourceMethod::HeaderCompletion(_) = method {
		unreachable!("HeaderCompletion request is not expected")
	}
}

fn source_reject_extra(method: &SourceMethod) {
	if let SourceMethod::HeaderExtra(_, _) = method {
		unreachable!("HeaderExtra request is not expected")
	}
}

fn target_accept_all_headers(method: &TargetMethod, data: &mut TargetData, requires_extra: bool) {
	if let TargetMethod::SubmitHeaders(ref submitted) = method {
		assert_eq!(submitted.iter().all(|header| header.extra().is_some()), requires_extra,);

		data.submit_headers_result = Some(SubmittedHeaders {
			submitted: submitted.iter().map(|header| header.id()).collect(),
			..Default::default()
		});
	}
}

fn target_signal_exit_when_header_submitted(
	method: &TargetMethod,
	header_id: TestHeaderId,
	exit_signal: &futures::channel::mpsc::UnboundedSender<()>,
) {
	if let TargetMethod::SubmitHeaders(ref submitted) = method {
		if submitted.iter().any(|header| header.id() == header_id) {
			exit_signal.unbounded_send(()).unwrap();
		}
	}
}

fn target_signal_exit_when_header_completed(
	method: &TargetMethod,
	header_id: TestHeaderId,
	exit_signal: &futures::channel::mpsc::UnboundedSender<()>,
) {
	if let TargetMethod::CompleteHeader(completed_id, _) = method {
		if *completed_id == header_id {
			exit_signal.unbounded_send(()).unwrap();
		}
	}
}

fn run_backoff_test(result: Result<(), TestError>) -> (Duration, Duration) {
	let mut backoff = retry_backoff();

	// no randomness in tests (otherwise intervals may overlap => asserts are failing)
	backoff.randomization_factor = 0f64;

	// increase backoff's current interval
	let interval1 = backoff.next_backoff().unwrap();
	let interval2 = backoff.next_backoff().unwrap();
	assert!(interval2 > interval1);

	// successful future result leads to backoff's reset
	let go_offline_future = futures::future::Fuse::terminated();
	futures::pin_mut!(go_offline_future);

	process_future_result(
		result,
		&mut backoff,
		|_| {},
		&mut go_offline_future,
		async_std::task::sleep,
		|| "Test error".into(),
	);

	(interval2, backoff.next_backoff().unwrap())
}

#[test]
fn process_future_result_resets_backoff_on_success() {
	let (interval2, interval_after_reset) = run_backoff_test(Ok(()));
	assert!(interval2 > interval_after_reset);
}

#[test]
fn process_future_result_resets_backoff_on_connection_error() {
	let (interval2, interval_after_reset) = run_backoff_test(Err(TestError(true)));
	assert!(interval2 > interval_after_reset);
}

#[test]
fn process_future_result_does_not_reset_backoff_on_non_connection_error() {
	let (interval2, interval_after_reset) = run_backoff_test(Err(TestError(false)));
	assert!(interval2 < interval_after_reset);
}

struct SyncLoopTestParams {
	best_source_header: TestHeader,
	headers_on_source: Vec<(bool, TestHeader)>,
	best_target_header: TestHeader,
	headers_on_target: Vec<TestHeader>,
	target_requires_extra: bool,
	target_requires_completion: bool,
	stop_at: TestHeaderId,
}

fn run_sync_loop_test(params: SyncLoopTestParams) {
	let (exit_sender, exit_receiver) = futures::channel::mpsc::unbounded();
	let target_requires_extra = params.target_requires_extra;
	let target_requires_completion = params.target_requires_completion;
	let stop_at = params.stop_at;
	let source = Source::new(
		params.best_source_header.id(),
		params.headers_on_source,
		move |method, _| {
			if !target_requires_extra {
				source_reject_extra(&method);
			}
			if !target_requires_completion {
				source_reject_completion(&method);
			}
		},
	);
	let target = Target::new(
		params.best_target_header.id(),
		params.headers_on_target.into_iter().map(|header| header.id()).collect(),
		move |method, data| {
			target_accept_all_headers(&method, data, target_requires_extra);
			if target_requires_completion {
				target_signal_exit_when_header_completed(&method, stop_at, &exit_sender);
			} else {
				target_signal_exit_when_header_submitted(&method, stop_at, &exit_sender);
			}
		},
	);
	target.data.lock().requires_extra = target_requires_extra;
	target.data.lock().requires_completion = target_requires_completion;

	run(
		source,
		test_tick(),
		target,
		test_tick(),
		(),
		crate::sync::tests::default_sync_params(),
		None,
		exit_receiver.into_future().map(|(_, _)| ()),
	);
}

#[test]
fn sync_loop_is_able_to_synchronize_single_header() {
	run_sync_loop_test(SyncLoopTestParams {
		best_source_header: test_header(1),
		headers_on_source: vec![(true, test_header(1))],
		best_target_header: test_header(0),
		headers_on_target: vec![test_header(0)],
		target_requires_extra: false,
		target_requires_completion: false,
		stop_at: test_id(1),
	});
}

#[test]
fn sync_loop_is_able_to_synchronize_single_header_with_extra() {
	run_sync_loop_test(SyncLoopTestParams {
		best_source_header: test_header(1),
		headers_on_source: vec![(true, test_header(1))],
		best_target_header: test_header(0),
		headers_on_target: vec![test_header(0)],
		target_requires_extra: true,
		target_requires_completion: false,
		stop_at: test_id(1),
	});
}

#[test]
fn sync_loop_is_able_to_synchronize_single_header_with_completion() {
	run_sync_loop_test(SyncLoopTestParams {
		best_source_header: test_header(1),
		headers_on_source: vec![(true, test_header(1))],
		best_target_header: test_header(0),
		headers_on_target: vec![test_header(0)],
		target_requires_extra: false,
		target_requires_completion: true,
		stop_at: test_id(1),
	});
}

#[test]
fn sync_loop_is_able_to_reorganize_from_shorter_fork() {
	run_sync_loop_test(SyncLoopTestParams {
		best_source_header: test_header(3),
		headers_on_source: vec![
			(true, test_header(1)),
			(true, test_header(2)),
			(true, test_header(3)),
			(false, test_forked_header(1, 0)),
			(false, test_forked_header(2, 0)),
		],
		best_target_header: test_forked_header(2, 0),
		headers_on_target: vec![test_header(0), test_forked_header(1, 0), test_forked_header(2, 0)],
		target_requires_extra: false,
		target_requires_completion: false,
		stop_at: test_id(3),
	});
}

#[test]
fn sync_loop_is_able_to_reorganize_from_longer_fork() {
	run_sync_loop_test(SyncLoopTestParams {
		best_source_header: test_header(3),
		headers_on_source: vec![
			(true, test_header(1)),
			(true, test_header(2)),
			(true, test_header(3)),
			(false, test_forked_header(1, 0)),
			(false, test_forked_header(2, 0)),
			(false, test_forked_header(3, 0)),
			(false, test_forked_header(4, 0)),
			(false, test_forked_header(5, 0)),
		],
		best_target_header: test_forked_header(5, 0),
		headers_on_target: vec![
			test_header(0),
			test_forked_header(1, 0),
			test_forked_header(2, 0),
			test_forked_header(3, 0),
			test_forked_header(4, 0),
			test_forked_header(5, 0),
		],
		target_requires_extra: false,
		target_requires_completion: false,
		stop_at: test_id(3),
	});
}
