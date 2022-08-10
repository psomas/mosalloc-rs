use libc::{c_int, c_void, intptr_t, off_t, ptrdiff_t, size_t};
use redhook::{hook, real};

use crate::allocator::Allocator;

use mosalloc::utils::htlb::MosallocConfig;

// mosalloc allocator instance when LD_PRELOAD hooks are used
static mut PRELOAD_ALLOC: Option<Allocator> = None;

// malloc __morecore hook for glibc<=2.33
extern "C" {
    static mut __morecore: extern "C" fn(intptr_t) -> *mut c_void;
}

// void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
hook! {
    unsafe fn mmap(addr: *mut c_void,
                   len: size_t,
                   prot: c_int,
                   flags: c_int,
                   fd: c_int,
                   offset: off_t) -> *mut c_void => mosalloc_mmap {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.mmap(addr as usize, len, prot, flags, fd, offset) as *mut c_void
        } else {
            real!(mmap)(addr, len, prot, flags, fd, offset)
        }
    }
}

pub fn libc_mmap(
    addr: *mut c_void,
    len: size_t,
    prot: c_int,
    flags: c_int,
    fd: c_int,
    offset: off_t,
) -> *mut c_void {
    unsafe { real!(mmap)(addr, len, prot, flags, fd, offset) }
}

// int munmap(void *addr, size_t length);
hook! {
    unsafe fn munmap(addr: *mut c_void,
                     len: size_t) -> c_int => mosalloc_munmap {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.munmap(addr as usize, len)
        } else {
            real!(munmap)(addr, len)
        }
    }
}

pub fn libc_munmap(addr: *mut c_void, len: size_t) -> c_int {
    unsafe { real!(munmap)(addr, len) }
}

// int mprotect(void *addr, size_t length, int prot);
hook! {
    unsafe fn mprotect(addr: *mut c_void,
                     len: size_t, prot: c_int) -> c_int => mosalloc_mprotect {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.mprotect(addr as usize, len, prot)
        } else {
            real!(mprotect)(addr, len, prot)
        }
    }
}

pub fn libc_mprotect(addr: *mut c_void, len: size_t, prot: c_int) -> c_int {
    unsafe { real!(mprotect)(addr, len, prot) }
}

// int madvise(void *addr, size_t length, int advice);
hook! {
    // FIXME: some of the madvise calls should be probably mocked by mosalloc for the mappings
    // under its control
    unsafe fn madvise(addr: *mut c_void,
                     len: size_t, advice: c_int) -> c_int => mosalloc_madvise {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.madvise(addr as usize, len, advice)
        } else {
            real!(madvise)(addr, len, advice)
        }
    }
}

pub fn libc_madvise(addr: *mut c_void, len: size_t, advice: c_int) -> c_int {
    unsafe { real!(madvise)(addr, len, advice) }
}

// void *mremap(void *old_address, size_t old_size, size_t new_size, int flags, ...)
hook! {
    // FIXME: handle mremap to mosalloc-managed mappings
    unsafe fn mremap(old_address: *mut c_void, old_size: size_t, new_size: size_t, flags: c_int, new_address: *mut c_void) -> *mut c_void => mosalloc_mremap {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.mremap(old_address as usize, old_size, new_size, flags, new_address as usize) as *mut c_void
        } else {
            real!(mremap)(old_address, old_size, new_size, flags, new_address)
        }
    }
}

pub fn libc_mremap(
    old_address: *mut c_void,
    old_size: size_t,
    new_size: size_t,
    flags: c_int,
    new_address: *mut c_void,
) -> *mut c_void {
    unsafe { real!(mremap)(old_address, old_size, new_size, flags, new_address) }
}

// int brk(void *addr);
hook! {
    unsafe fn brk(addr: *mut c_void) -> c_int => mosalloc_brk {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.brk(addr as usize)
        } else {
            real!(brk)(addr)
        }
    }
}

pub fn libc_brk(addr: *mut c_void) -> c_int {
    println!("{:x}", addr as usize);
    unsafe { real!(brk)(addr) }
}

// void *sbrk(intptr_t increment);
hook! {
    unsafe fn sbrk(incr: intptr_t) -> *mut c_void => mosalloc_sbrk {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.sbrk(incr) as *mut c_void
        } else {
            real!(sbrk)(incr)
        }
    }
}

pub fn libc_sbrk(incr: intptr_t) -> *mut c_void {
    unsafe { real!(sbrk)(incr) }
}

#[no_mangle]
pub extern "C" fn mosalloc_morecore(incr: ptrdiff_t) -> *mut c_void {
    unsafe {
        if let Some(mosalloc) = PRELOAD_ALLOC.as_mut() {
            mosalloc.sbrk(incr) as *mut c_void
        } else {
            real!(sbrk)(incr)
        }
    }
}

pub unsafe fn preload_init(config: MosallocConfig) {
    __morecore = mosalloc_morecore as extern "C" fn(intptr_t) -> *mut c_void;

    PRELOAD_ALLOC = Some(Allocator::new(config, false));
    PRELOAD_ALLOC.as_mut().unwrap().drain();
}
