use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::ptr::{copy_nonoverlapping, null_mut, write_bytes};
use std::sync::atomic::{AtomicUsize, Ordering};

use libc;

use crate::preload_hooks;

use mosalloc::utils::misc::align_up;
use mosalloc::pr_dbg;

const ARENA_SIZE: usize = 256 * 1024;
const MAX_SUPPORTED_ALIGN: usize = 4096;
const MMAP_THRESHOLD: usize = 4096;

/// Internal alloator for libmosalloc / Rust internal allocations.
/// Based on the simple example allocator in GlobalAlloc documentation.
/// Uses a small statically allocated arena for the small allocations and
/// falls back to mmap (page-sized) allocations for larger requests.
/// The static arena only supports freeing from the top.
#[repr(C, align(4096))]
pub struct InternalAllocator {
    arena: UnsafeCell<[u8; ARENA_SIZE]>,
    idx: AtomicUsize,
    mmap_total: AtomicUsize,
    mmap_overhead: AtomicUsize,
}

#[global_allocator]
static INTERNAL_ALLOCATOR: InternalAllocator = InternalAllocator {
    arena: UnsafeCell::new([0; ARENA_SIZE]),
    idx: AtomicUsize::new(0),
    mmap_total: AtomicUsize::new(0),
    mmap_overhead: AtomicUsize::new(0),
};

unsafe impl Sync for InternalAllocator {}

impl InternalAllocator {
    pub unsafe fn print_stats() {
        let arena_total = INTERNAL_ALLOCATOR.idx.load(Ordering::Relaxed);
        let arena_rem = ARENA_SIZE - arena_total;
        pr_dbg!(
            "(arena) allocated: {:.02}KB, remaining: {:.02}KB",
            arena_total as f64 / 1024.0,
            arena_rem as f64 / 1024.0
        );

        let mmap_total = INTERNAL_ALLOCATOR.mmap_total.load(Ordering::Relaxed);
        let mmap_overhead = INTERNAL_ALLOCATOR.mmap_overhead.load(Ordering::Relaxed);
        pr_dbg!(
            "(mmap) allocated: {:.02}MB, overhead: {:.02}KB",
            mmap_total as f64 / 1024.0 / 1024.0,
            mmap_overhead as f64 / 1024.0
        );
    }

    fn mmap_alloc(&self, size: usize) -> *mut u8 {
        preload_hooks::libc_mmap(
            null_mut() as *mut _,
            size,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
            -1,
            0,
        ) as *mut u8
    }

    unsafe fn alloc_helper(&self, layout: Layout, zero: bool) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();

        if align > MAX_SUPPORTED_ALIGN {
            return null_mut();
        }

        if size >= MMAP_THRESHOLD {
            self.mmap_total.fetch_add(size, Ordering::Relaxed);
            self.mmap_overhead
                .fetch_add(align_up(size, 4096) - size, Ordering::Relaxed);

            return self.mmap_alloc(size) as *mut u8;
        }

        match self
            .idx
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |mut idx| {
                idx = align_up(idx, align);
                let new_idx = idx.checked_add(size).unwrap();
                if new_idx > ARENA_SIZE {
                    return None;
                }
                Some(new_idx)
            }) {
            Ok(prev_idx) => {
                let ptr = (self.arena.get() as *mut u8).add(align_up(prev_idx, align));
                if zero {
                    write_bytes(ptr, 0, size);
                }
                ptr
            }
            Err(_) => null_mut(),
        }
    }
}

unsafe impl GlobalAlloc for InternalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_helper(layout, false)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();

        if size >= MMAP_THRESHOLD {
            self.mmap_total.fetch_sub(size, Ordering::Relaxed);
            self.mmap_overhead
                .fetch_sub(align_up(size, 4096) - size, Ordering::Relaxed);

            assert_eq!(preload_hooks::libc_munmap(ptr as *mut _, layout.size()), 0);
            return;
        }

        self.idx
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |idx| {
                let top = (self.arena.get() as *mut u8).add(idx);
                if ptr.add(size) != top {
                    return None;
                }

                Some(idx - size)
            })
            .unwrap_or(0);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.alloc_helper(layout, true)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_size = layout.size();
        let align = layout.align();

        if (old_size >= MMAP_THRESHOLD) ^ (new_size >= MMAP_THRESHOLD) {
            let new_ptr = self.alloc(Layout::from_size_align(new_size, align).unwrap());
            copy_nonoverlapping(ptr, new_ptr, old_size.min(new_size));
            self.dealloc(ptr, layout);
            return new_ptr;
        }

        if old_size >= MMAP_THRESHOLD {
            self.mmap_total.fetch_sub(old_size, Ordering::Relaxed);
            self.mmap_overhead
                .fetch_sub(align_up(old_size, 4096) - old_size, Ordering::Relaxed);
            self.mmap_total.fetch_add(new_size, Ordering::Relaxed);
            self.mmap_overhead
                .fetch_add(align_up(new_size, 4096) - new_size, Ordering::Relaxed);

            let ret = libc::mremap(ptr as *mut _, old_size, new_size, libc::MREMAP_MAYMOVE);
            assert!(ret != libc::MAP_FAILED);
            return ret as *mut u8;
        } else {
            if self
                .idx
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |idx| {
                    let top = (self.arena.get() as *mut u8).add(idx);
                    if ptr.add(old_size) != top {
                        return None;
                    }

                    if new_size < old_size {
                        return Some(idx - (old_size - new_size));
                    }

                    if idx + (new_size - old_size) > ARENA_SIZE {
                        return None;
                    }

                    Some(idx + (new_size - old_size))
                })
                .is_err()
            {
                let new_ptr = self.alloc(Layout::from_size_align(new_size, align).unwrap());
                copy_nonoverlapping(ptr, new_ptr, old_size.min(new_size));
                new_ptr
            } else {
                ptr
            }
        }
    }
}
