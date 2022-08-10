use ctor::ctor;
use libc::{__errno_location, c_int, c_void, intptr_t, off_t, size_t, EINVAL, ENOMEM, MAP_FAILED};
use redhook::{hook, real};
use std::hint::black_box;
use std::ptr::null;

use crate::allocator::Allocator;
use crate::internal_allocator::InternalAllocator;

pub static mut ALLOC: Option<Allocator> = None;
pub static mut DRAINED: bool = false;

const CHUNK: usize = 64;

// void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
hook! {
    unsafe fn mmap(addr: *mut c_void,
                   len: size_t,
                   prot: c_int,
                   flags: c_int,
                   fd: c_int,
                   offset: off_t) -> *mut c_void => mosalloc_mmap {
        if let Some(mosalloc) = ALLOC.as_mut() {
            if DRAINED == false {
                *__errno_location() = ENOMEM;
                MAP_FAILED as *mut c_void
            } else {
                mosalloc.mmap(addr as usize, len, prot, flags, fd, offset) as *mut c_void
            }
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
        if let Some(mosalloc) = ALLOC.as_mut() {
            if DRAINED == false {
                *__errno_location() = EINVAL;
                -1
            } else {
                mosalloc.munmap(addr as usize, len)
            }
        } else {
            real!(munmap)(addr, len)
        }
    }
}

pub fn libc_munmap(addr: *mut c_void, len: size_t) -> c_int {
    unsafe { real!(munmap)(addr, len) }
}

// int brk(void *addr);
hook! {
    unsafe fn brk(addr: *mut c_void) -> c_int => mosalloc_brk {
        if let Some(mosalloc) = ALLOC.as_mut() {
            if DRAINED == false {
                *__errno_location() = ENOMEM;
                -1
            } else if mosalloc.brk(addr as usize) == -1 {
                *__errno_location() = ENOMEM;
                -1
            } else {
                0
            }
        } else {
            real!(brk)(addr)
        }
    }
}

pub fn libc_brk(addr: *mut c_void) -> c_int {
    unsafe { real!(brk)(addr) }
}

// void *sbrk(intptr_t increment);
hook! {
    unsafe fn sbrk(incr: intptr_t) -> *mut c_void => mosalloc_sbrk {
        if let Some(mosalloc) = ALLOC.as_mut() {
            if DRAINED == false {
                *__errno_location() = ENOMEM;
                return usize::MAX as *mut c_void;
            }

            let ret = mosalloc.sbrk(incr);
            if ret == usize::MAX  {
                *__errno_location() = ENOMEM;
            }
            ret as *mut c_void
        } else {
            real!(sbrk)(incr)
        }
    }
}

pub fn libc_sbrk(incr: intptr_t) -> *mut c_void {
    unsafe { real!(sbrk)(incr) }
}

#[ctor]
unsafe fn activate_mosalloc() {
    ALLOC = Some(Allocator::new());

    while black_box(libc::malloc(CHUNK)) as *const u8 != null() {}
    *__errno_location() = 0;
    DRAINED = true;

    InternalAllocator::print_overhead();
}
