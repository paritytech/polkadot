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

//! Implements API for managing a ring buffer and an associated message window.

use frame_support::pallet_prelude::*;
use primitives::v2::Id as ParaId;
use sp_std::prelude::*;

/// A type of index that wraps around. It is used for messages and pages.
#[derive(
	Encode, Decode, Default, Clone, Copy, sp_runtime::RuntimeDebug, Eq, PartialEq, TypeInfo,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct WrappingIndex(u64);

impl WrappingIndex {
	/// Wrapping addition between two indexes.
	pub fn wrapping_add(&self, other: WrappingIndex) -> Self {
		WrappingIndex(self.0.wrapping_add(other.0))
	}

	/// Wrapping substraction between two indexes.
	pub fn wrapping_sub(&self, other: WrappingIndex) -> Self {
		WrappingIndex(self.0.wrapping_sub(other.0))
	}

	/// Wrapping increment.
	pub fn wrapping_inc(&self) -> Self {
		WrappingIndex(self.0.wrapping_add(1))
	}

	/// Wrapping decrement.
	pub fn wrapping_dec(&self) -> Self {
		WrappingIndex(self.0.wrapping_sub(1))
	}
}

impl From<u64> for WrappingIndex {
	fn from(idx: u64) -> Self {
		WrappingIndex(idx)
	}
}

impl Into<u64> for WrappingIndex {
	fn into(self) -> u64 {
		self.0
	}
}

/// Unique identifier of an inbound downward message.
#[derive(Encode, Decode, Clone, Default, Copy, sp_runtime::RuntimeDebug, PartialEq, TypeInfo)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageIdx {
	/// The recipient parachain.
	pub para_id: ParaId,
	/// A message index in the recipient parachain queue.
	pub message_idx: WrappingIndex,
}

/// The key for a queue page of a parachain.
#[derive(Encode, Decode, Clone, Copy, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct QueuePageIdx {
	/// The recipient parachain.
	pub para_id: ParaId,
	/// The page index.
	pub page_idx: WrappingIndex,
}

/// The state of the message window. The message window is used to provide a 1:1 mapping to the
/// messages stored in the ring buffer.
///
/// Invariants:
/// - the window size is always equal to the amount of messages stored in the ring buffer.
#[derive(Encode, Decode, Default, Clone, Copy, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct MessageWindowState {
	/// The first used index corresponding to first message in the queue.
	first_message_idx: WrappingIndex,
	/// The first free index.
	free_message_idx: WrappingIndex,
}

/// The state of the ring buffer that represents the message queue. We only need to keep track
/// of the first used(head) and unused(tail) pages.
/// Invariants:
///  - the window size is always equal to the queue size.
#[derive(Encode, Decode, Default, Clone, Copy, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct RingBufferState {
	/// The index of the first used page.
	head_page_idx: WrappingIndex,
	/// The index of the first unused page. `tail_page_idx - 1` is the last used page.
	tail_page_idx: WrappingIndex,
}

#[cfg(test)]
impl RingBufferState {
	pub fn new(head_page_idx: WrappingIndex, tail_page_idx: WrappingIndex) -> RingBufferState {
		RingBufferState { tail_page_idx, head_page_idx }
	}
}

/// Manages the downward message indexing window. All downward messages are assigned
/// an index when they are queued.
pub struct MessageWindow {
	para_id: ParaId,
	state: MessageWindowState,
}

#[derive(Clone, Copy)]
/// Provides basic methods to interact with the ring buffer.
pub struct RingBuffer {
	para_id: ParaId,
	state: RingBufferState,
}

/// An iterator over the collection of pages in the ring buffer.
pub struct RingBufferIterator(RingBuffer);

impl IntoIterator for RingBuffer {
	type Item = QueuePageIdx;
	type IntoIter = RingBufferIterator;

	fn into_iter(self) -> Self::IntoIter {
		RingBufferIterator(self)
	}
}

impl Iterator for RingBufferIterator {
	type Item = QueuePageIdx;

	fn next(&mut self) -> Option<Self::Item> {
		self.0.pop_front()
	}
}

impl RingBuffer {
	pub fn with_state(state: RingBufferState, para_id: ParaId) -> RingBuffer {
		RingBuffer { state, para_id }
	}

	/// Allocates a new page and returns the page index.
	/// Panics if there are no free pages.
	pub fn extend(&mut self) -> QueuePageIdx {
		// In practice this is always bounded economically - sending one requires a fee.
		if self.state.tail_page_idx.wrapping_inc() == self.state.head_page_idx {
			unimplemented!("The end of the world is upon us");
		}

		// Advance tail to the next unused page.
		self.state.tail_page_idx = self.state.tail_page_idx.wrapping_inc();
		// Return last used page.
		QueuePageIdx { para_id: self.para_id, page_idx: self.state.tail_page_idx.wrapping_dec() }
	}

	/// Allocates a new page and returns the page index.
	pub fn prune(&mut self, count: u32) {
		// Ensure we don't overflow and the head overtakes the tail.
		let to_prune = sp_std::cmp::min(self.size(), count as u64);

		// Advance tail by `count` elements.
		self.state.head_page_idx = self.state.head_page_idx.wrapping_add(to_prune.into());
	}

	/// Frees the first used page and returns it's index while advacing the head of the ring buffer.
	/// If the queue is empty it does nothing and returns `None`.
	pub fn pop_front(&mut self) -> Option<QueuePageIdx> {
		let page = self.front();

		if page.is_some() {
			self.state.head_page_idx = self.state.head_page_idx.wrapping_inc();
		}

		page
	}

	/// Returns the first page or `None` if ring buffer empty.
	pub fn front(&self) -> Option<QueuePageIdx> {
		if self.state.tail_page_idx == self.state.head_page_idx {
			None
		} else {
			Some(QueuePageIdx { para_id: self.para_id, page_idx: self.state.head_page_idx })
		}
	}

	/// Returns the last used page or `None` if ring buffer empty.
	pub fn last_used(&self) -> Option<QueuePageIdx> {
		if self.state.tail_page_idx == self.state.head_page_idx {
			None
		} else {
			Some(QueuePageIdx {
				para_id: self.para_id,
				page_idx: self.state.tail_page_idx.wrapping_dec(),
			})
		}
	}

	#[cfg(test)]
	pub fn first_unused(&self) -> QueuePageIdx {
		QueuePageIdx { para_id: self.para_id, page_idx: self.state.tail_page_idx }
	}

	/// Returns the size in pages.
	pub fn size(&self) -> u64 {
		self.state.tail_page_idx.wrapping_sub(self.state.head_page_idx).into()
	}

	/// Returns the wrapped state.
	pub fn into_inner(self) -> RingBufferState {
		self.state
	}
}

impl MessageWindow {
	/// Construct from state of a given para.
	pub fn with_state(state: MessageWindowState, para_id: ParaId) -> MessageWindow {
		MessageWindow { para_id, state }
	}

	/// Extend the message index window by `count`. Returns the latest used message index.
	/// Panics if extending over capacity, similarly to `RingBuffer`.
	pub fn extend(&mut self, count: u64) -> MessageIdx {
		self.state.free_message_idx = self.state.free_message_idx.wrapping_add(count.into());
		MessageIdx {
			para_id: self.para_id,
			message_idx: self.state.free_message_idx.wrapping_dec(),
		}
	}

	/// Advanced the window start by `count` elements.  Returns the index of the first element in queue
	/// or `None` if the queue is empty after the operation.
	pub fn prune(&mut self, count: u64) -> Option<MessageIdx> {
		let to_prune = sp_std::cmp::min(self.size(), count);
		self.state.first_message_idx = self.state.first_message_idx.wrapping_add(to_prune.into());
		if self.state.first_message_idx == self.state.free_message_idx {
			None
		} else {
			Some(MessageIdx { para_id: self.para_id, message_idx: self.state.first_message_idx })
		}
	}

	/// Returns the size of the message window.
	pub fn size(&self) -> u64 {
		self.state.free_message_idx.wrapping_sub(self.state.first_message_idx).into()
	}

	/// Returns the first message index, `None` if window is empty.
	pub fn first(&self) -> Option<MessageIdx> {
		if self.size() > 0 {
			Some(MessageIdx { para_id: self.para_id, message_idx: self.state.first_message_idx })
		} else {
			None
		}
	}

	/// Returns the first free message index.
	pub fn first_free(&self) -> MessageIdx {
		MessageIdx { para_id: self.para_id, message_idx: self.state.free_message_idx }
	}

	/// Returns the wrapped state.
	pub fn into_inner(self) -> MessageWindowState {
		self.state
	}
}

#[cfg(test)]
mod test {}
