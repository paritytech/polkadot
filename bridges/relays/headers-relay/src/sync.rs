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

//! Headers synchronization context. This structure wraps headers queue and is
//! able to choose: which headers to read from the source chain? Which headers
//! to submit to the target chain? The context makes decisions basing on parameters
//! passed using `HeadersSyncParams` structure.

use crate::headers::QueuedHeaders;
use crate::sync_types::{HeaderIdOf, HeaderStatus, HeadersSyncPipeline, QueuedHeader};
use num_traits::{One, Saturating, Zero};

/// Common sync params.
#[derive(Debug, Clone)]
pub struct HeadersSyncParams {
	/// Maximal number of ethereum headers to pre-download.
	pub max_future_headers_to_download: usize,
	/// Maximal number of active (we believe) submit header transactions.
	pub max_headers_in_submitted_status: usize,
	/// Maximal number of headers in single submit request.
	pub max_headers_in_single_submit: usize,
	/// Maximal total headers size in single submit request.
	pub max_headers_size_in_single_submit: usize,
	/// We only may store and accept (from Ethereum node) headers that have
	/// number >= than best_substrate_header.number - prune_depth.
	pub prune_depth: u32,
	/// Target transactions mode.
	pub target_tx_mode: TargetTransactionMode,
}

/// Target transaction mode.
#[derive(Debug, PartialEq, Clone)]
pub enum TargetTransactionMode {
	/// Submit new headers using signed transactions.
	Signed,
	/// Submit new headers using unsigned transactions.
	Unsigned,
	/// Submit new headers using signed transactions, but only when we
	/// believe that sync has stalled.
	Backup,
}

/// Headers synchronization context.
#[derive(Debug)]
pub struct HeadersSync<P: HeadersSyncPipeline> {
	/// Synchronization parameters.
	params: HeadersSyncParams,
	/// Best header number known to source node.
	source_best_number: Option<P::Number>,
	/// Best header known to target node.
	target_best_header: Option<HeaderIdOf<P>>,
	/// Headers queue.
	headers: QueuedHeaders<P>,
	/// Pause headers submission.
	pause_submit: bool,
}

impl<P: HeadersSyncPipeline> HeadersSync<P> {
	/// Creates new headers synchronizer.
	pub fn new(params: HeadersSyncParams) -> Self {
		HeadersSync {
			headers: QueuedHeaders::default(),
			params,
			source_best_number: None,
			target_best_header: None,
			pause_submit: false,
		}
	}

	/// Return best header number known to source node.
	pub fn source_best_number(&self) -> Option<P::Number> {
		self.source_best_number
	}

	/// Best header known to target node.
	pub fn target_best_header(&self) -> Option<HeaderIdOf<P>> {
		self.target_best_header
	}

	/// Returns true if we have synced almost all known headers.
	pub fn is_almost_synced(&self) -> bool {
		match self.source_best_number {
			Some(source_best_number) => self
				.target_best_header
				.map(|best| source_best_number.saturating_sub(best.0) < 4.into())
				.unwrap_or(false),
			None => true,
		}
	}

	/// Returns synchronization status.
	pub fn status(&self) -> (&Option<HeaderIdOf<P>>, &Option<P::Number>) {
		(&self.target_best_header, &self.source_best_number)
	}

	/// Returns reference to the headers queue.
	pub fn headers(&self) -> &QueuedHeaders<P> {
		&self.headers
	}

	/// Returns mutable reference to the headers queue.
	pub fn headers_mut(&mut self) -> &mut QueuedHeaders<P> {
		&mut self.headers
	}

	/// Select header that needs to be downloaded from the source node.
	pub fn select_new_header_to_download(&self) -> Option<P::Number> {
		// if we haven't received best header from source node yet, there's nothing we can download
		let source_best_number = self.source_best_number?;

		// if we haven't received known best header from target node yet, there's nothing we can download
		let target_best_header = self.target_best_header.as_ref()?;

		// if there's too many headers in the queue, stop downloading
		let in_memory_headers = self.headers.total_headers();
		if in_memory_headers >= self.params.max_future_headers_to_download {
			return None;
		}

		// if queue is empty and best header on target is > than best header on source,
		// then we shoud reorg
		let best_queued_number = self.headers.best_queued_number();
		if best_queued_number.is_zero() && source_best_number < target_best_header.0 {
			return Some(source_best_number);
		}

		// we assume that there were no reorgs if we have already downloaded best header
		let best_downloaded_number = std::cmp::max(
			std::cmp::max(best_queued_number, self.headers.best_synced_number()),
			target_best_header.0,
		);
		if best_downloaded_number >= source_best_number {
			return None;
		}

		// download new header
		Some(best_downloaded_number + One::one())
	}

	/// Selech orphan header to downoload.
	pub fn select_orphan_header_to_download(&self) -> Option<&QueuedHeader<P>> {
		let orphan_header = self.headers.header(HeaderStatus::Orphan)?;

		// we consider header orphan until we'll find it ancestor that is known to the target node
		// => we may get orphan header while we ask target node whether it knows its parent
		// => let's avoid fetching duplicate headers
		let parent_id = orphan_header.parent_id();
		if self.headers.status(&parent_id) != HeaderStatus::Unknown {
			return None;
		}

		Some(orphan_header)
	}

	/// Select headers that need to be submitted to the target node.
	pub fn select_headers_to_submit(&self, stalled: bool) -> Option<Vec<&QueuedHeader<P>>> {
		// maybe we have paused new headers submit?
		if self.pause_submit {
			return None;
		}

		// if we operate in backup mode, we only submit headers when sync has stalled
		if self.params.target_tx_mode == TargetTransactionMode::Backup && !stalled {
			return None;
		}

		let headers_in_submit_status = self.headers.headers_in_status(HeaderStatus::Submitted);
		let headers_to_submit_count = self
			.params
			.max_headers_in_submitted_status
			.checked_sub(headers_in_submit_status)?;

		let mut total_size = 0;
		let mut total_headers = 0;
		self.headers.headers(HeaderStatus::Ready, |header| {
			if total_headers == headers_to_submit_count {
				return false;
			}
			if total_headers == self.params.max_headers_in_single_submit {
				return false;
			}

			let encoded_size = P::estimate_size(header);
			if total_headers != 0 && total_size + encoded_size > self.params.max_headers_size_in_single_submit {
				return false;
			}

			total_size += encoded_size;
			total_headers += 1;

			true
		})
	}

	/// Receive new target header number from the source node.
	pub fn source_best_header_number_response(&mut self, best_header_number: P::Number) {
		log::debug!(
			target: "bridge",
			"Received best header number from {} node: {}",
			P::SOURCE_NAME,
			best_header_number,
		);
		self.source_best_number = Some(best_header_number);
	}

	/// Receive new best header from the target node.
	/// Returns true if it is different from the previous block known to us.
	pub fn target_best_header_response(&mut self, best_header: HeaderIdOf<P>) -> bool {
		log::debug!(
			target: "bridge",
			"Received best known header from {}: {:?}",
			P::TARGET_NAME,
			best_header,
		);

		// early return if it is still the same
		if self.target_best_header == Some(best_header) {
			return false;
		}

		// remember that this header is now known to the Substrate runtime
		self.headers.target_best_header_response(&best_header);

		// prune ancient headers
		self.headers
			.prune(best_header.0.saturating_sub(self.params.prune_depth.into()));

		// finally remember the best header itself
		self.target_best_header = Some(best_header);

		// we are ready to submit headers again
		if self.pause_submit {
			log::debug!(
				target: "bridge",
				"Ready to submit {} headers to {} node again!",
				P::SOURCE_NAME,
				P::TARGET_NAME,
			);

			self.pause_submit = false;
		}

		true
	}

	/// Pause headers submit until best header will be updated on target node.
	pub fn pause_submit(&mut self) {
		log::debug!(
			target: "bridge",
			"Stopping submitting {} headers to {} node. Waiting for {} submitted headers to be accepted",
			P::SOURCE_NAME,
			P::TARGET_NAME,
			self.headers.headers_in_status(HeaderStatus::Submitted),
		);

		self.pause_submit = true;
	}

	/// Restart synchronization.
	pub fn restart(&mut self) {
		self.source_best_number = None;
		self.target_best_header = None;
		self.headers.clear();
		self.pause_submit = false;
	}
}

#[cfg(test)]
pub mod tests {
	use super::*;
	use crate::headers::tests::{header, id};
	use crate::sync_loop_tests::{TestHash, TestHeadersSyncPipeline, TestNumber};
	use crate::sync_types::HeaderStatus;
	use relay_utils::HeaderId;

	fn side_hash(number: TestNumber) -> TestHash {
		1000 + number
	}

	pub fn default_sync_params() -> HeadersSyncParams {
		HeadersSyncParams {
			max_future_headers_to_download: 128,
			max_headers_in_submitted_status: 128,
			max_headers_in_single_submit: 32,
			max_headers_size_in_single_submit: 131_072,
			prune_depth: 4096,
			target_tx_mode: TargetTransactionMode::Signed,
		}
	}

	#[test]
	fn select_new_header_to_download_works() {
		let mut eth_sync = HeadersSync::<TestHeadersSyncPipeline>::new(default_sync_params());

		// both best && target headers are unknown
		assert_eq!(eth_sync.select_new_header_to_download(), None);

		// best header is known, target header is unknown
		eth_sync.target_best_header = Some(HeaderId(0, Default::default()));
		assert_eq!(eth_sync.select_new_header_to_download(), None);

		// target header is known, best header is unknown
		eth_sync.target_best_header = None;
		eth_sync.source_best_number = Some(100);
		assert_eq!(eth_sync.select_new_header_to_download(), None);

		// when our best block has the same number as the target
		eth_sync.target_best_header = Some(HeaderId(100, Default::default()));
		assert_eq!(eth_sync.select_new_header_to_download(), None);

		// when we actually need a new header
		eth_sync.source_best_number = Some(101);
		assert_eq!(eth_sync.select_new_header_to_download(), Some(101));

		// when we have to reorganize to longer fork
		eth_sync.source_best_number = Some(100);
		eth_sync.target_best_header = Some(HeaderId(200, Default::default()));
		assert_eq!(eth_sync.select_new_header_to_download(), Some(100));

		// when there are too many headers scheduled for submitting
		for i in 1..1000 {
			eth_sync.headers.header_response(header(i).header().clone());
		}
		assert_eq!(eth_sync.select_new_header_to_download(), None);
	}

	#[test]
	fn select_new_header_to_download_works_with_empty_queue() {
		let mut eth_sync = HeadersSync::<TestHeadersSyncPipeline>::new(default_sync_params());
		eth_sync.source_best_header_number_response(100);

		// when queue is not empty => everything goes as usually
		eth_sync.target_best_header_response(header(10).id());
		eth_sync.headers_mut().header_response(header(11).header().clone());
		eth_sync.headers_mut().maybe_extra_response(&header(11).id(), false);
		assert_eq!(eth_sync.select_new_header_to_download(), Some(12));

		// but then queue is drained
		eth_sync.headers_mut().target_best_header_response(&header(11).id());

		// even though it's empty, we know that header#11 is synced
		assert_eq!(eth_sync.headers().best_queued_number(), 0);
		assert_eq!(eth_sync.headers().best_synced_number(), 11);
		assert_eq!(eth_sync.select_new_header_to_download(), Some(12));
	}

	#[test]
	fn sync_without_reorgs_works() {
		let mut eth_sync = HeadersSync::new(default_sync_params());
		eth_sync.params.max_headers_in_submitted_status = 1;

		// ethereum reports best header #102
		eth_sync.source_best_header_number_response(102);

		// substrate reports that it is at block #100
		eth_sync.target_best_header_response(id(100));

		// block #101 is downloaded first
		assert_eq!(eth_sync.select_new_header_to_download(), Some(101));
		eth_sync.headers.header_response(header(101).header().clone());

		// now header #101 is ready to be submitted
		assert_eq!(eth_sync.headers.header(HeaderStatus::MaybeExtra), Some(&header(101)));
		eth_sync.headers.maybe_extra_response(&id(101), false);
		assert_eq!(eth_sync.headers.header(HeaderStatus::Ready), Some(&header(101)));
		assert_eq!(eth_sync.select_headers_to_submit(false), Some(vec![&header(101)]));

		// and header #102 is ready to be downloaded
		assert_eq!(eth_sync.select_new_header_to_download(), Some(102));
		eth_sync.headers.header_response(header(102).header().clone());

		// receive submission confirmation
		eth_sync.headers.headers_submitted(vec![id(101)]);

		// we have nothing to submit because previous header hasn't been confirmed yet
		// (and we allow max 1 submit transaction in the wild)
		assert_eq!(eth_sync.headers.header(HeaderStatus::MaybeExtra), Some(&header(102)));
		eth_sync.headers.maybe_extra_response(&id(102), false);
		assert_eq!(eth_sync.headers.header(HeaderStatus::Ready), Some(&header(102)));
		assert_eq!(eth_sync.select_headers_to_submit(false), None);

		// substrate reports that it has imported block #101
		eth_sync.target_best_header_response(id(101));

		// and we are ready to submit #102
		assert_eq!(eth_sync.select_headers_to_submit(false), Some(vec![&header(102)]));
		eth_sync.headers.headers_submitted(vec![id(102)]);

		// substrate reports that it has imported block #102
		eth_sync.target_best_header_response(id(102));

		// and we have nothing to download
		assert_eq!(eth_sync.select_new_header_to_download(), None);
	}

	#[test]
	fn sync_with_orphan_headers_work() {
		let mut eth_sync = HeadersSync::new(default_sync_params());

		// ethereum reports best header #102
		eth_sync.source_best_header_number_response(102);

		// substrate reports that it is at block #100, but it isn't part of best chain
		eth_sync.target_best_header_response(HeaderId(100, side_hash(100)));

		// block #101 is downloaded first
		assert_eq!(eth_sync.select_new_header_to_download(), Some(101));
		eth_sync.headers.header_response(header(101).header().clone());

		// we can't submit header #101, because its parent status is unknown
		assert_eq!(eth_sync.select_headers_to_submit(false), None);

		// instead we are trying to determine status of its parent (#100)
		assert_eq!(eth_sync.headers.header(HeaderStatus::MaybeOrphan), Some(&header(101)));

		// and the status is still unknown
		eth_sync.headers.maybe_orphan_response(&id(100), false);

		// so we consider #101 orphaned now && will download its parent - #100
		assert_eq!(eth_sync.headers.header(HeaderStatus::Orphan), Some(&header(101)));
		eth_sync.headers.header_response(header(100).header().clone());

		// #101 is now Orphan and #100 is MaybeOrphan => we do not want to retrieve
		// header #100 again
		assert_eq!(eth_sync.headers.header(HeaderStatus::Orphan), Some(&header(101)));
		assert_eq!(eth_sync.select_orphan_header_to_download(), None);

		// we can't submit header #100, because its parent status is unknown
		assert_eq!(eth_sync.select_headers_to_submit(false), None);

		// instead we are trying to determine status of its parent (#99)
		assert_eq!(eth_sync.headers.header(HeaderStatus::MaybeOrphan), Some(&header(100)));

		// and the status is known, so we move previously orphaned #100 and #101 to ready queue
		eth_sync.headers.maybe_orphan_response(&id(99), true);

		// and we are ready to submit #100
		assert_eq!(eth_sync.headers.header(HeaderStatus::MaybeExtra), Some(&header(100)));
		eth_sync.headers.maybe_extra_response(&id(100), false);
		assert_eq!(eth_sync.select_headers_to_submit(false), Some(vec![&header(100)]));
		eth_sync.headers.headers_submitted(vec![id(100)]);

		// and we are ready to submit #101
		assert_eq!(eth_sync.headers.header(HeaderStatus::MaybeExtra), Some(&header(101)));
		eth_sync.headers.maybe_extra_response(&id(101), false);
		assert_eq!(eth_sync.select_headers_to_submit(false), Some(vec![&header(101)]));
		eth_sync.headers.headers_submitted(vec![id(101)]);
	}

	#[test]
	fn pruning_happens_on_target_best_header_response() {
		let mut eth_sync = HeadersSync::<TestHeadersSyncPipeline>::new(default_sync_params());
		eth_sync.params.prune_depth = 50;
		eth_sync.target_best_header_response(id(100));
		assert_eq!(eth_sync.headers.prune_border(), 50);
	}

	#[test]
	fn only_submitting_headers_in_backup_mode_when_stalled() {
		let mut eth_sync = HeadersSync::new(default_sync_params());
		eth_sync.params.target_tx_mode = TargetTransactionMode::Backup;

		// ethereum reports best header #102
		eth_sync.source_best_header_number_response(102);

		// substrate reports that it is at block #100
		eth_sync.target_best_header_response(id(100));

		// block #101 is downloaded first
		eth_sync.headers.header_response(header(101).header().clone());
		eth_sync.headers.maybe_extra_response(&id(101), false);

		// ensure that headers are not submitted when sync is not stalled
		assert_eq!(eth_sync.select_headers_to_submit(false), None);

		// ensure that headers are not submitted when sync is stalled
		assert_eq!(eth_sync.select_headers_to_submit(true), Some(vec![&header(101)]));
	}

	#[test]
	fn does_not_select_new_headers_to_submit_when_submit_is_paused() {
		let mut eth_sync = HeadersSync::new(default_sync_params());
		eth_sync.params.max_headers_in_submitted_status = 1;

		// ethereum reports best header #102 and substrate is at #100
		eth_sync.source_best_header_number_response(102);
		eth_sync.target_best_header_response(id(100));

		// let's prepare #101 and #102 for submitting
		eth_sync.headers.header_response(header(101).header().clone());
		eth_sync.headers.maybe_extra_response(&id(101), false);
		eth_sync.headers.header_response(header(102).header().clone());
		eth_sync.headers.maybe_extra_response(&id(102), false);

		// when submit is not paused, we're ready to submit #101
		assert_eq!(eth_sync.select_headers_to_submit(false), Some(vec![&header(101)]));

		// when submit is paused, we're not ready to submit anything
		eth_sync.pause_submit();
		assert_eq!(eth_sync.select_headers_to_submit(false), None);

		// if best header on substrate node isn't updated, we still not submitting anything
		eth_sync.target_best_header_response(id(100));
		assert_eq!(eth_sync.select_headers_to_submit(false), None);

		// but after it is actually updated, we are ready to submit
		eth_sync.target_best_header_response(id(101));
		assert_eq!(eth_sync.select_headers_to_submit(false), Some(vec![&header(102)]));
	}
}
