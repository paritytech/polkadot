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

//! Substrate-to-Substrate headers synchronization maintain procedure.
//!
//! Regular headers synchronization only depends on persistent justifications
//! that are generated when authorities set changes. This happens rarely on
//! real-word chains. So some other way to finalize headers is required.
//!
//! Full nodes are listening to GRANDPA messages, so they may have track authorities
//! votes on their own. They're returning both persistent and ephemeral justifications
//! (justifications that are not stored in the database and not broadcasted over network)
//! throught `grandpa_subscribeJustifications` RPC subscription.
//!
//! The idea of this maintain procedure is that when we see justification that 'improves'
//! best finalized header on the target chain, we submit this justification to the target
//! node.

use crate::headers_pipeline::SubstrateHeadersSyncPipeline;

use async_std::sync::{Arc, Mutex};
use async_trait::async_trait;
use codec::{Decode, Encode};
use futures::future::{poll_fn, FutureExt, TryFutureExt};
use headers_relay::{
	sync::HeadersSync,
	sync_loop::SyncMaintain,
	sync_types::{HeaderIdOf, HeaderStatus},
};
use relay_substrate_client::{Chain, Client, Error as SubstrateError, JustificationsSubscription};
use relay_utils::HeaderId;
use sp_core::Bytes;
use sp_runtime::{traits::Header as HeaderT, Justification};
use std::{collections::VecDeque, marker::PhantomData, task::Poll};

/// Substrate-to-Substrate headers synchronization maintain procedure.
pub struct SubstrateHeadersToSubstrateMaintain<P: SubstrateHeadersSyncPipeline, SourceChain, TargetChain: Chain> {
	pipeline: P,
	target_client: Client<TargetChain>,
	justifications: Arc<Mutex<Justifications<P>>>,
	_marker: PhantomData<SourceChain>,
}

/// Future and already received justifications from the source chain.
struct Justifications<P: SubstrateHeadersSyncPipeline> {
	/// Justifications stream.
	stream: JustificationsSubscription,
	/// Justifications that we have read from the stream but have not sent to the
	/// target node, because their targets were still not synced.
	queue: VecDeque<(HeaderIdOf<P>, Justification)>,
}

impl<P: SubstrateHeadersSyncPipeline, SourceChain, TargetChain: Chain>
	SubstrateHeadersToSubstrateMaintain<P, SourceChain, TargetChain>
{
	/// Create new maintain procedure.
	pub fn new(pipeline: P, target_client: Client<TargetChain>, justifications: JustificationsSubscription) -> Self {
		SubstrateHeadersToSubstrateMaintain {
			pipeline,
			target_client,
			justifications: Arc::new(Mutex::new(Justifications {
				stream: justifications,
				queue: VecDeque::new(),
			})),
			_marker: Default::default(),
		}
	}
}

#[async_trait]
impl<P: SubstrateHeadersSyncPipeline, SourceChain, TargetChain: Chain> Clone
	for SubstrateHeadersToSubstrateMaintain<P, SourceChain, TargetChain>
{
	fn clone(&self) -> Self {
		SubstrateHeadersToSubstrateMaintain {
			pipeline: self.pipeline.clone(),
			target_client: self.target_client.clone(),
			justifications: self.justifications.clone(),
			_marker: Default::default(),
		}
	}
}

#[async_trait]
impl<P, SourceChain, TargetChain> SyncMaintain<P> for SubstrateHeadersToSubstrateMaintain<P, SourceChain, TargetChain>
where
	SourceChain: Chain,
	<SourceChain::Header as HeaderT>::Number: Into<P::Number>,
	<SourceChain::Header as HeaderT>::Hash: Into<P::Hash>,
	TargetChain: Chain,
	P::Number: Decode,
	P::Hash: Decode,
	P: SubstrateHeadersSyncPipeline<Completion = Justification, Extra = ()>,
{
	async fn maintain(&self, sync: &mut HeadersSync<P>) {
		// lock justifications before doing anything else
		let mut justifications = match self.justifications.try_lock() {
			Some(justifications) => justifications,
			None => {
				// this should never happen, as we use single-thread executor
				log::warn!(target: "bridge", "Failed to acquire {} justifications lock", P::SOURCE_NAME);
				return;
			}
		};

		// we need to read best finalized header from the target node to be able to
		// choose justification to submit
		let best_finalized = match best_finalized_header_id::<P, _>(&self.target_client).await {
			Ok(best_finalized) => best_finalized,
			Err(error) => {
				log::warn!(
					target: "bridge",
					"Failed to read best finalized {} block from maintain: {:?}",
					P::SOURCE_NAME,
					error,
				);
				return;
			}
		};

		log::debug!(
			target: "bridge",
			"Read best finalized {} block from {}: {:?}",
			P::SOURCE_NAME,
			P::TARGET_NAME,
			best_finalized,
		);

		// Select justification to submit to the target node. We're submitting at most one justification
		// on every maintain call. So maintain rate directly affects finalization rate.
		let justification_to_submit = poll_fn(|context| {
			// read justifications from the stream and push to the queue
			justifications.read_from_stream::<SourceChain::Header>(context);

			// remove all obsolete justifications from the queue
			remove_obsolete::<P>(&mut justifications.queue, best_finalized);

			// select justification to submit
			Poll::Ready(select_justification(&mut justifications.queue, sync))
		})
		.await;

		// finally - submit selected justification
		if let Some((target, justification)) = justification_to_submit {
			let submit_result = self
				.pipeline
				.make_complete_header_transaction(target, justification)
				.and_then(|tx| self.target_client.submit_extrinsic(Bytes(tx.encode())))
				.await;

			match submit_result {
				Ok(_) => log::debug!(
					target: "bridge",
					"Submitted justification received over {} subscription. Target: {:?}",
					P::SOURCE_NAME,
					target,
				),
				Err(error) => log::warn!(
					target: "bridge",
					"Failed to submit justification received over {} subscription for {:?}: {:?}",
					P::SOURCE_NAME,
					target,
					error,
				),
			}
		}
	}
}

impl<P> Justifications<P>
where
	P::Number: Decode,
	P::Hash: Decode,
	P: SubstrateHeadersSyncPipeline<Completion = Justification, Extra = ()>,
{
	/// Read justifications from the subscription stream without blocking.
	fn read_from_stream<'a, SourceHeader>(&mut self, context: &mut std::task::Context<'a>)
	where
		SourceHeader: HeaderT,
		SourceHeader::Number: Into<P::Number>,
		SourceHeader::Hash: Into<P::Hash>,
	{
		loop {
			let maybe_next_justification = self.stream.next();
			futures::pin_mut!(maybe_next_justification);

			let maybe_next_justification = maybe_next_justification.poll_unpin(context);
			let justification = match maybe_next_justification {
				Poll::Ready(justification) => justification,
				Poll::Pending => return,
			};

			// decode justification target
			let target = bp_header_chain::justification::decode_justification_target::<SourceHeader>(&justification);
			let target = match target {
				Ok((target_hash, target_number)) => HeaderId(target_number.into(), target_hash.into()),
				Err(error) => {
					log::warn!(
						target: "bridge",
						"Failed to decode justification from {} subscription: {:?}",
						P::SOURCE_NAME,
						error,
					);
					continue;
				}
			};

			log::debug!(
				target: "bridge",
				"Received {} justification over subscription. Target: {:?}",
				P::SOURCE_NAME,
				target,
			);

			self.queue.push_back((target, justification.0));
		}
	}
}

/// Clean queue of all justifications that are justifying already finalized blocks.
fn remove_obsolete<P: SubstrateHeadersSyncPipeline>(
	queue: &mut VecDeque<(HeaderIdOf<P>, Justification)>,
	best_finalized: HeaderIdOf<P>,
) {
	while queue
		.front()
		.map(|(target, _)| target.0 <= best_finalized.0)
		.unwrap_or(false)
	{
		queue.pop_front();
	}
}

/// Select appropriate justification that would improve best finalized block on target node.
///
/// It is assumed that the selected justification will be submitted to the target node. The
/// justification itself and all preceeding justifications are removed from the queue.
fn select_justification<P>(
	queue: &mut VecDeque<(HeaderIdOf<P>, Justification)>,
	sync: &mut HeadersSync<P>,
) -> Option<(HeaderIdOf<P>, Justification)>
where
	P: SubstrateHeadersSyncPipeline<Completion = Justification>,
{
	let mut selected_justification = None;
	while let Some((target, justification)) = queue.pop_front() {
		// if we're waiting for this justification, report it
		if sync.headers().requires_completion_data(&target) {
			sync.headers_mut().completion_response(&target, Some(justification));
			// we won't submit previous justifications as we going to submit justification for
			// next header
			selected_justification = None;
			// we won't submit next justifications as we need to submit previous justifications
			// first
			break;
		}

		// if we know that the header is already synced (it is known to the target node), let's
		// select it for submission. We still may select better justification on the next iteration.
		if sync.headers().status(&target) == HeaderStatus::Synced {
			selected_justification = Some((target, justification));
			continue;
		}

		// finally - return justification back to the queue
		queue.push_back((target, justification));
		break;
	}

	selected_justification
}

/// Returns best finalized source header on the target chain.
async fn best_finalized_header_id<P, C>(client: &Client<C>) -> Result<HeaderIdOf<P>, SubstrateError>
where
	P: SubstrateHeadersSyncPipeline,
	P::Number: Decode,
	P::Hash: Decode,
	C: Chain,
{
	let call = P::FINALIZED_BLOCK_METHOD.into();
	let data = Bytes(Vec::new());

	let encoded_response = client.state_call(call, data, None).await?;
	let decoded_response: (P::Number, P::Hash) =
		Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?;

	let best_header_id = HeaderId(decoded_response.0, decoded_response.1);
	Ok(best_header_id)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::headers_pipeline::sync_params;
	use crate::millau_headers_to_rialto::MillauHeadersToRialto;

	fn parent_hash(index: u8) -> bp_millau::Hash {
		if index == 1 {
			Default::default()
		} else {
			header(index - 1).hash()
		}
	}

	fn header_hash(index: u8) -> bp_millau::Hash {
		header(index).hash()
	}

	fn header(index: u8) -> bp_millau::Header {
		bp_millau::Header::new(
			index as _,
			Default::default(),
			Default::default(),
			parent_hash(index),
			Default::default(),
		)
	}

	#[test]
	fn obsolete_justifications_are_removed() {
		let mut queue = vec![
			(HeaderId(1, header_hash(1)), vec![1]),
			(HeaderId(2, header_hash(2)), vec![2]),
			(HeaderId(3, header_hash(3)), vec![3]),
		]
		.into_iter()
		.collect();

		remove_obsolete::<MillauHeadersToRialto>(&mut queue, HeaderId(2, header_hash(2)));

		assert_eq!(
			queue,
			vec![(HeaderId(3, header_hash(3)), vec![3])]
				.into_iter()
				.collect::<VecDeque<_>>(),
		);
	}

	#[test]
	fn latest_justification_is_selected() {
		let mut queue = vec![
			(HeaderId(1, header_hash(1)), vec![1]),
			(HeaderId(2, header_hash(2)), vec![2]),
			(HeaderId(3, header_hash(3)), vec![3]),
		]
		.into_iter()
		.collect();
		let mut sync = HeadersSync::<MillauHeadersToRialto>::new(sync_params());
		sync.headers_mut().header_response(header(1).into());
		sync.headers_mut().header_response(header(2).into());
		sync.headers_mut().header_response(header(3).into());
		sync.target_best_header_response(HeaderId(2, header_hash(2)));

		assert_eq!(
			select_justification(&mut queue, &mut sync),
			Some((HeaderId(2, header_hash(2)), vec![2])),
		);
	}

	#[test]
	fn required_justification_is_reported() {
		let mut queue = vec![
			(HeaderId(1, header_hash(1)), vec![1]),
			(HeaderId(2, header_hash(2)), vec![2]),
			(HeaderId(3, header_hash(3)), vec![3]),
		]
		.into_iter()
		.collect();
		let mut sync = HeadersSync::<MillauHeadersToRialto>::new(sync_params());
		sync.headers_mut().header_response(header(1).into());
		sync.headers_mut().header_response(header(2).into());
		sync.headers_mut().header_response(header(3).into());
		sync.headers_mut()
			.incomplete_headers_response(vec![HeaderId(2, header_hash(2))].into_iter().collect());
		sync.target_best_header_response(HeaderId(2, header_hash(2)));

		assert_eq!(sync.headers_mut().header_to_complete(), None,);

		assert_eq!(select_justification(&mut queue, &mut sync), None,);

		assert_eq!(
			sync.headers_mut().header_to_complete(),
			Some((HeaderId(2, header_hash(2)), &vec![2])),
		);
	}
}
