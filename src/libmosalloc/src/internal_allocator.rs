use std::alloc::{GlobalAlloc, Layout};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use libc;

use crate::hooks;

use mosalloc::utils::htlb::PAGE_SIZE;
use mosalloc::utils::misc::{align_up, is_aligned, size_to_str};

static OVERHEAD: AtomicUsize = AtomicUsize::new(0);
static MAX_OVERHEAD: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static MAX_TOTAL: AtomicUsize = AtomicUsize::new(0);

/// mmap-based allocator for internal (Rust) libmosalloc allocations.
///
/// Due to its page granularity, it introduces some overhead, but the
/// overall Rust allocations ammount to just ~2MB with most of this memory being
/// used by the allocation and placement maps (similar to the
/// first-fit-allocator structures in the original C++ mosalloc), which are
/// generally page aligned. The total overhead / waste from the page allocations
/// seems to be ~2000-300KB.
pub struct InternalAllocator;

impl InternalAllocator {
    // internal allocator statistics
    pub fn print_overhead() {
        println!("libmosalloc internal (Rust) allocations: current {} / max {}, (mmap allocator) overhead current {} / max {}", 
                 size_to_str(TOTAL.load(SeqCst)), size_to_str(MAX_TOTAL.load(SeqCst)),
                 size_to_str(OVERHEAD.load(SeqCst)), size_to_str(MAX_OVERHEAD.load(SeqCst)));
    }
}

unsafe impl GlobalAlloc for InternalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ret = hooks::libc_mmap(
            null_mut() as *mut libc::c_void,
            align_up(layout.size(), PAGE_SIZE),
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
            -1,
            0,
        ) as *mut u8;

        if !ret.is_null() {
            OVERHEAD.fetch_add(align_up(layout.size(), PAGE_SIZE) - layout.size(), SeqCst);
            TOTAL.fetch_add(layout.size(), SeqCst);
        }
        return ret;
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        assert!(is_aligned(ptr as usize, PAGE_SIZE));
        assert!(
            hooks::libc_munmap(ptr as *mut libc::c_void, align_up(layout.size(), PAGE_SIZE)) == 0
        );
        MAX_OVERHEAD.fetch_max(
            OVERHEAD.fetch_sub(align_up(layout.size(), PAGE_SIZE) - layout.size(), SeqCst),
            SeqCst,
        );
        MAX_TOTAL.fetch_max(TOTAL.fetch_sub(layout.size(), SeqCst), SeqCst);
    }
}

#[global_allocator]
static INTERNAL: InternalAllocator = InternalAllocator;
