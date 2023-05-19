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

//! Tracking global allocator. Calculates the peak allocation between two checkpoints.

use core::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, Ordering};
use tikv_jemallocator::Jemalloc;

struct TrackingAllocatorData {
	lock: AtomicBool,
	current: isize,
	peak: isize,
}

impl TrackingAllocatorData {
	#[inline]
	fn lock(&self) {
		loop {
			// Try to acquire the lock.
			if self
				.lock
				.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
				.is_ok()
			{
				break
			}
			// We failed to acquire the lock; wait until it's unlocked.
			//
			// In theory this should result in less coherency traffic as unlike `compare_exchange`
			// it is a read-only operation, so multiple cores can execute it simultaneously
			// without taking an exclusive lock over the cache line.
			while self.lock.load(Ordering::Relaxed) {
				std::hint::spin_loop();
			}
		}
	}

	#[inline]
	fn unlock(&self) {
		self.lock.store(false, Ordering::Release);
	}

	fn start_tracking(&mut self) {
		self.lock();
		self.current = 0;
		self.peak = 0;
		self.unlock();
	}

	fn end_tracking(&self) -> isize {
		self.lock();
		let peak = self.peak;
		self.unlock();
		peak
	}

	#[inline]
	fn track(&mut self, alloc: isize) {
		self.lock();
		self.current += alloc;
		if self.current > self.peak {
			self.peak = self.current;
		}
		self.unlock();
	}
}

static mut ALLOCATOR_DATA: TrackingAllocatorData =
	TrackingAllocatorData { lock: AtomicBool::new(false), current: 0, peak: 0 };

pub struct TrackingAllocator<A: GlobalAlloc>(A);

impl<A: GlobalAlloc> TrackingAllocator<A> {
	// SAFETY:
	// * The following functions write to `static mut`. That is safe as the critical section
	//   inside is isolated by an exclusive lock.

	/// Start tracking
	pub fn start_tracking(&self) {
		unsafe {
			ALLOCATOR_DATA.start_tracking();
		}
	}

	/// End tracking and return the peak allocation value in bytes (as `isize`). Peak allocation
	/// value is not guaranteed to be neither non-zero nor positive.
	pub fn end_tracking(&self) -> isize {
		unsafe { ALLOCATOR_DATA.end_tracking() }
	}
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for TrackingAllocator<A> {
	// SAFETY:
	// * The wrapped methods are as safe as the underlying allocator implementation is

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
pub static ALLOC: TrackingAllocator<Jemalloc> = TrackingAllocator(Jemalloc);
