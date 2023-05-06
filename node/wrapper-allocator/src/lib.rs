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

use core::sync::atomic::{ Ordering::SeqCst, AtomicUsize };
use core::alloc::{GlobalAlloc, Layout};
use tikv_jemallocator::Jemalloc;

/// 
pub struct WrapperAllocatorData {
    allocated: AtomicUsize,
    checkpoint: AtomicUsize,
    peak: AtomicUsize,
    // limit: AtomicUsize, // Should we introduce a checkpoint limit and fail allocation if the limit is hit?
}

impl WrapperAllocatorData {
    /// Marks a new checkpoint. Returns peak allocation, in bytes, since the last checkpoint.
    pub fn checkpoint(&self) -> usize {
        let alloc = ALLOCATOR_DATA.allocated.load(SeqCst);
        let old_cp = ALLOCATOR_DATA.checkpoint.swap(alloc, SeqCst);
        ALLOCATOR_DATA.peak.swap(alloc, SeqCst).saturating_sub(old_cp)
    }
}

pub static ALLOCATOR_DATA: WrapperAllocatorData = WrapperAllocatorData { allocated: AtomicUsize::new(0), checkpoint: AtomicUsize::new(0), peak: AtomicUsize::new(0) };

struct WrapperAllocator<A: GlobalAlloc>(A);

unsafe impl<A: GlobalAlloc> GlobalAlloc for WrapperAllocator<A> {

    // SAFETY: The wrapped methods are as safe as the underlying allocator implementation is.

    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let old_alloc = ALLOCATOR_DATA.allocated.fetch_add(layout.size(), SeqCst);
        ALLOCATOR_DATA.peak.fetch_max(old_alloc + layout.size(), SeqCst);
        self.0.alloc(layout)
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let old_alloc = ALLOCATOR_DATA.allocated.fetch_add(layout.size(), SeqCst);
        ALLOCATOR_DATA.peak.fetch_max(old_alloc + layout.size(), SeqCst);
        self.0.alloc_zeroed(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) -> () {
        ALLOCATOR_DATA.allocated.fetch_sub(layout.size(), SeqCst);
        self.0.dealloc(ptr, layout)
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() {
            let old_alloc = ALLOCATOR_DATA.allocated.fetch_add(new_size - layout.size(), SeqCst);
            ALLOCATOR_DATA.peak.fetch_max(old_alloc + new_size - layout.size(), SeqCst);
        } else {
            ALLOCATOR_DATA.allocated.fetch_sub(layout.size() - new_size, SeqCst);
        }
        self.0.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: WrapperAllocator<Jemalloc> = WrapperAllocator(Jemalloc);
