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

//! Tracking global allocator. Initially just forwards allocation and deallocation requests
//! to the underlying allocator. When tracking is enabled, stores every allocation event into
//! pre-allocated backlog. When tracking mode is disables, replays the backlog and counts the
//! number of allocation events and the peak allocation value.

use core::alloc::{GlobalAlloc, Layout};
use std::sync::{
	atomic::{AtomicUsize, Ordering::Relaxed},
	RwLock,
};
use tikv_jemallocator::Jemalloc;

struct WrapperAllocatorData {
	tracking: RwLock<bool>,
	backlog: Vec<isize>,
	backlog_index: AtomicUsize,
}

impl WrapperAllocatorData {
	// SAFETY:
	// * Tracking must only be performed by a single thread at a time
	// * `start_tracking` and `stop_tracking` must be called from the same thread
	// * Tracking periods must not overlap
	// * Caller must provide sufficient backlog size

	unsafe fn start_tracking(&mut self, backlog_size: usize) {
		// Allocate the backlog before locking anything. The allocation won't be available later.
		let backlog = Vec::with_capacity(backlog_size);
		// Lock allocations, move the allocated vector to our place and start tracking.
		let mut tracking = self.tracking.write().unwrap();
		assert!(!*tracking); // Shouldn't start tracking if already tracking
		self.backlog = backlog;
		self.backlog.resize(backlog_size, 0);
		self.backlog_index.store(0, Relaxed);
		*tracking = true;
	}

	unsafe fn end_tracking(&mut self) -> (usize, isize) {
		let mut tracking = self.tracking.write().unwrap();
		assert!(*tracking); // Start/end calls must be consistent
        
		// At this point, all the allocation is blocked as all the threads are waiting for
		// read lock on `tracking`. The following code replays the backlog and calulates the
		// peak value. It must not perform any allocation, otherwise a deadlock will occur.
		let mut peak = 0;
		let mut alloc = 0;
		let mut events = 0usize;
		for i in 0..self.backlog.len() {
			if self.backlog[i] == 0 {
				break
			}
			events += 1;
			alloc += self.backlog[i];
			if alloc > peak {
				peak = alloc
			}
		}
		*tracking = false;
		(events, peak)
	}

	#[inline]
	unsafe fn track(&mut self, alloc: isize) {
		let tracking = self.tracking.read().unwrap();
		if !*tracking {
			return
		}
		let i = self.backlog_index.fetch_add(1, Relaxed);
		if i == self.backlog.len() {
			// We cannot use formatted text here as it would result in allocations and a deadlock
			panic!("Backlog size provided was not enough for allocation tracking");
		}
		// It is safe as the vector is pre-allocated and the index is acquired atomically
		self.backlog[i] = alloc;
	}
}

static mut ALLOCATOR_DATA: WrapperAllocatorData = WrapperAllocatorData {
	tracking: RwLock::new(false),
	backlog: vec![],
	backlog_index: AtomicUsize::new(0),
};

pub struct WrapperAllocator<A: GlobalAlloc>(A);

impl<A: GlobalAlloc> WrapperAllocator<A> {
	/// Start tracking with the given backlog size (in allocation events). Providing insufficient
	/// backlog size will result in a panic.
	pub fn start_tracking(&self, backlog_size: usize) {
		unsafe {
			ALLOCATOR_DATA.start_tracking(backlog_size);
		}
	}

	/// End tracking and return number of allocation events (as `usize`) and peak allocation
	/// value in bytes (as `isize`). Peak allocation value is not guaranteed to be neither
	/// non-zero nor positive.
	pub fn end_tracking(&self) -> (usize, isize) {
		unsafe { ALLOCATOR_DATA.end_tracking() }
	}
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for WrapperAllocator<A> {
	// SAFETY:
	// * The wrapped methods are as safe as the underlying allocator implementation is
	// * In tracking mode, it is safe as long as a sufficient backlog size is provided when
	//   entering the tracking mode

	#[inline]
	unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
		ALLOCATOR_DATA.track(layout.size() as isize);
		self.0.alloc(layout)
	}

	#[inline]
	unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
		ALLOCATOR_DATA.track(layout.size() as isize);
		self.0.alloc_zeroed(layout)
	}

	#[inline]
	unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) -> () {
		ALLOCATOR_DATA.track(-(layout.size() as isize));
		self.0.dealloc(ptr, layout)
	}

	#[inline]
	unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
		ALLOCATOR_DATA.track((new_size as isize) - (layout.size() as isize));
		self.0.realloc(ptr, layout, new_size)
	}
}

#[global_allocator]
pub static ALLOC: WrapperAllocator<Jemalloc> = WrapperAllocator(Jemalloc);
