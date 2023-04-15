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

//! A utility for fetching all unknown blocks based on a new chain-head hash.

use futures::{channel::oneshot, prelude::*};
use polkadot_node_subsystem::{messages::ChainApiMessage, SubsystemSender};
use polkadot_primitives::{BlockNumber, Hash, Header};

/// Given a new chain-head hash, this determines the hashes of all new blocks we should track
/// metadata for, given this head.
///
/// This is guaranteed to be a subset of the (inclusive) ancestry of `head` determined as all
/// blocks above the lower bound or above the highest known block, whichever is higher.
/// This is formatted in descending order by block height.
///
/// An implication of this is that if `head` itself is known or not above the lower bound,
/// then the returned list will be empty.
///
/// This may be somewhat expensive when first recovering from major sync.
pub async fn determine_new_blocks<E, Sender>(
	sender: &mut Sender,
	is_known: impl Fn(&Hash) -> Result<bool, E>,
	head: Hash,
	header: &Header,
	lower_bound_number: BlockNumber,
) -> Result<Vec<(Hash, Header)>, E>
where
	Sender: SubsystemSender<ChainApiMessage>,
{
	const ANCESTRY_STEP: usize = 4;

	let min_block_needed = lower_bound_number + 1;

	// Early exit if the block is in the DB or too early.
	{
		let already_known = is_known(&head)?;

		let before_relevant = header.number < min_block_needed;

		if already_known || before_relevant {
			return Ok(Vec::new())
		}
	}

	let mut ancestry = vec![(head, header.clone())];

	// Early exit if the parent hash is in the DB or no further blocks
	// are needed.
	if is_known(&header.parent_hash)? || header.number == min_block_needed {
		return Ok(ancestry)
	}

	'outer: loop {
		let (last_hash, last_header) = ancestry
			.last()
			.expect("ancestry has length 1 at initialization and is only added to; qed");

		assert!(
			last_header.number > min_block_needed,
			"Loop invariant: the last block in ancestry is checked to be \
			above the minimum before the loop, and at the end of each iteration; \
			qed"
		);

		let (tx, rx) = oneshot::channel();

		// This is always non-zero as determined by the loop invariant
		// above.
		let ancestry_step =
			std::cmp::min(ANCESTRY_STEP, (last_header.number - min_block_needed) as usize);

		let batch_hashes = if ancestry_step == 1 {
			vec![last_header.parent_hash]
		} else {
			sender
				.send_message(
					ChainApiMessage::Ancestors {
						hash: *last_hash,
						k: ancestry_step,
						response_channel: tx,
					}
					.into(),
				)
				.await;

			// Continue past these errors.
			match rx.await {
				Err(_) | Ok(Err(_)) => break 'outer,
				Ok(Ok(ancestors)) => ancestors,
			}
		};

		let batch_headers = {
			let (batch_senders, batch_receivers) = (0..batch_hashes.len())
				.map(|_| oneshot::channel())
				.unzip::<_, _, Vec<_>, Vec<_>>();

			for (hash, batched_sender) in batch_hashes.iter().cloned().zip(batch_senders) {
				sender
					.send_message(ChainApiMessage::BlockHeader(hash, batched_sender).into())
					.await;
			}

			let mut requests = futures::stream::FuturesOrdered::new();
			batch_receivers
				.into_iter()
				.map(|rx| async move {
					match rx.await {
						Err(_) | Ok(Err(_)) => None,
						Ok(Ok(h)) => h,
					}
				})
				.for_each(|x| requests.push_back(x));

			let batch_headers: Vec<_> =
				requests.flat_map(|x: Option<Header>| stream::iter(x)).collect().await;

			// Any failed header fetch of the batch will yield a `None` result that will
			// be skipped. Any failure at this stage means we'll just ignore those blocks
			// as the chain DB has failed us.
			if batch_headers.len() != batch_hashes.len() {
				break 'outer
			}
			batch_headers
		};

		for (hash, header) in batch_hashes.into_iter().zip(batch_headers) {
			let is_known = is_known(&hash)?;

			let is_relevant = header.number >= min_block_needed;
			let is_terminating = header.number == min_block_needed;

			if is_known || !is_relevant {
				break 'outer
			}

			ancestry.push((hash, header));

			if is_terminating {
				break 'outer
			}
		}
	}

	Ok(ancestry)
}

#[cfg(test)]
mod tests {
	use super::*;
	use assert_matches::assert_matches;
	use polkadot_node_subsystem_test_helpers::make_subsystem_context;
	use polkadot_overseer::{AllMessages, SubsystemContext};
	use sp_core::testing::TaskExecutor;
	use std::collections::{HashMap, HashSet};

	#[derive(Default)]
	struct TestKnownBlocks {
		blocks: HashSet<Hash>,
	}

	impl TestKnownBlocks {
		fn insert(&mut self, hash: Hash) {
			self.blocks.insert(hash);
		}

		fn is_known(&self, hash: &Hash) -> Result<bool, ()> {
			Ok(self.blocks.contains(hash))
		}
	}

	#[derive(Clone)]
	struct TestChain {
		start_number: BlockNumber,
		headers: Vec<Header>,
		numbers: HashMap<Hash, BlockNumber>,
	}

	impl TestChain {
		fn new(start: BlockNumber, len: usize) -> Self {
			assert!(len > 0, "len must be at least 1");

			let base = Header {
				digest: Default::default(),
				extrinsics_root: Default::default(),
				number: start,
				state_root: Default::default(),
				parent_hash: Default::default(),
			};

			let base_hash = base.hash();

			let mut chain = TestChain {
				start_number: start,
				headers: vec![base],
				numbers: vec![(base_hash, start)].into_iter().collect(),
			};

			for _ in 1..len {
				chain.grow()
			}

			chain
		}

		fn grow(&mut self) {
			let next = {
				let last = self.headers.last().unwrap();
				Header {
					digest: Default::default(),
					extrinsics_root: Default::default(),
					number: last.number + 1,
					state_root: Default::default(),
					parent_hash: last.hash(),
				}
			};

			self.numbers.insert(next.hash(), next.number);
			self.headers.push(next);
		}

		fn header_by_number(&self, number: BlockNumber) -> Option<&Header> {
			if number < self.start_number {
				None
			} else {
				self.headers.get((number - self.start_number) as usize)
			}
		}

		fn header_by_hash(&self, hash: &Hash) -> Option<&Header> {
			self.numbers.get(hash).and_then(|n| self.header_by_number(*n))
		}

		fn hash_by_number(&self, number: BlockNumber) -> Option<Hash> {
			self.header_by_number(number).map(|h| h.hash())
		}

		fn ancestry(&self, hash: &Hash, k: BlockNumber) -> Vec<Hash> {
			let n = match self.numbers.get(hash) {
				None => return Vec::new(),
				Some(&n) => n,
			};

			(0..k)
				.map(|i| i + 1)
				.filter_map(|i| self.header_by_number(n - i))
				.map(|h| h.hash())
				.collect()
		}
	}

	#[test]
	fn determine_new_blocks_back_to_lower_bound() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let known = TestKnownBlocks::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let lower_bound_number = 12;

		// Finalized block should be omitted. The head provided to `determine_new_blocks`
		// should be included.
		let expected_ancestry = (13..=18)
			.map(|n| chain.header_by_number(n).map(|h| (h.hash(), h.clone())).unwrap())
			.rev()
			.collect::<Vec<_>>();

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				ctx.sender(),
				|h| known.is_known(h),
				head_hash,
				&head,
				lower_bound_number,
			)
			.await
			.unwrap();

			assert_eq!(ancestry, expected_ancestry);
		});

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: h,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(h, head_hash);
					assert_eq!(k, 4);
					let _ = tx.send(Ok(chain.ancestry(&h, k as _)));
				}
			);

			for _ in 0u32..4 {
				assert_matches!(
					handle.recv().await,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
						let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
					}
				);
			}

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
					assert_eq!(h, chain.hash_by_number(13).unwrap());
					let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn determine_new_blocks_back_to_known() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut known = TestKnownBlocks::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let lower_bound_number = 12;
		let known_number = 15;
		let known_hash = chain.hash_by_number(known_number).unwrap();

		known.insert(known_hash);

		// Known block should be omitted. The head provided to `determine_new_blocks`
		// should be included.
		let expected_ancestry = (16..=18)
			.map(|n| chain.header_by_number(n).map(|h| (h.hash(), h.clone())).unwrap())
			.rev()
			.collect::<Vec<_>>();

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				ctx.sender(),
				|h| known.is_known(h),
				head_hash,
				&head,
				lower_bound_number,
			)
			.await
			.unwrap();

			assert_eq!(ancestry, expected_ancestry);
		});

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: h,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(h, head_hash);
					assert_eq!(k, 4);
					let _ = tx.send(Ok(chain.ancestry(&h, k as _)));
				}
			);

			for _ in 0u32..4 {
				assert_matches!(
					handle.recv().await,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
						let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn determine_new_blocks_already_known_is_empty() {
		let pool = TaskExecutor::new();
		let (mut ctx, _handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut known = TestKnownBlocks::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let lower_bound_number = 0;

		known.insert(head_hash);

		// Known block should be omitted.
		let expected_ancestry = Vec::new();

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				ctx.sender(),
				|h| known.is_known(h),
				head_hash,
				&head,
				lower_bound_number,
			)
			.await
			.unwrap();

			assert_eq!(ancestry, expected_ancestry);
		});

		futures::executor::block_on(test_fut);
	}

	#[test]
	fn determine_new_blocks_parent_known_is_fast() {
		let pool = TaskExecutor::new();
		let (mut ctx, _handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut known = TestKnownBlocks::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let lower_bound_number = 0;
		let parent_hash = chain.hash_by_number(17).unwrap();

		known.insert(parent_hash);

		// New block should be the only new one.
		let expected_ancestry = vec![(head_hash, head.clone())];

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				ctx.sender(),
				|h| known.is_known(h),
				head_hash,
				&head,
				lower_bound_number,
			)
			.await
			.unwrap();

			assert_eq!(ancestry, expected_ancestry);
		});

		futures::executor::block_on(test_fut);
	}

	#[test]
	fn determine_new_block_before_finality_is_empty() {
		let pool = TaskExecutor::new();
		let (mut ctx, _handle) = make_subsystem_context::<(), _>(pool.clone());

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let parent_hash = chain.hash_by_number(17).unwrap();
		let mut known = TestKnownBlocks::default();

		known.insert(parent_hash);

		let test_fut = Box::pin(async move {
			let after_finality =
				determine_new_blocks(ctx.sender(), |h| known.is_known(h), head_hash, &head, 17)
					.await
					.unwrap();

			let at_finality =
				determine_new_blocks(ctx.sender(), |h| known.is_known(h), head_hash, &head, 18)
					.await
					.unwrap();

			let before_finality =
				determine_new_blocks(ctx.sender(), |h| known.is_known(h), head_hash, &head, 19)
					.await
					.unwrap();

			assert_eq!(after_finality, vec![(head_hash, head.clone())]);

			assert_eq!(at_finality, Vec::new());

			assert_eq!(before_finality, Vec::new());
		});

		futures::executor::block_on(test_fut);
	}

	#[test]
	fn determine_new_blocks_does_not_request_genesis() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let chain = TestChain::new(1, 2);

		let head = chain.header_by_number(2).unwrap().clone();
		let head_hash = head.hash();
		let known = TestKnownBlocks::default();

		let expected_ancestry = (1..=2)
			.map(|n| chain.header_by_number(n).map(|h| (h.hash(), h.clone())).unwrap())
			.rev()
			.collect::<Vec<_>>();

		let test_fut = Box::pin(async move {
			let ancestry =
				determine_new_blocks(ctx.sender(), |h| known.is_known(h), head_hash, &head, 0)
					.await
					.unwrap();

			assert_eq!(ancestry, expected_ancestry);
		});

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
					assert_eq!(h, chain.hash_by_number(1).unwrap());
					let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn determine_new_blocks_does_not_request_genesis_even_in_multi_ancestry() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let chain = TestChain::new(1, 3);

		let head = chain.header_by_number(3).unwrap().clone();
		let head_hash = head.hash();
		let known = TestKnownBlocks::default();

		let expected_ancestry = (1..=3)
			.map(|n| chain.header_by_number(n).map(|h| (h.hash(), h.clone())).unwrap())
			.rev()
			.collect::<Vec<_>>();

		let test_fut = Box::pin(async move {
			let ancestry =
				determine_new_blocks(ctx.sender(), |h| known.is_known(h), head_hash, &head, 0)
					.await
					.unwrap();

			assert_eq!(ancestry, expected_ancestry);
		});

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: h,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(h, head_hash);
					assert_eq!(k, 2);

					let _ = tx.send(Ok(chain.ancestry(&h, k as _)));
				}
			);

			for _ in 0_u8..2 {
				assert_matches!(
					handle.recv().await,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
						let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}
}
