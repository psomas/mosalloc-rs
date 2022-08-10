use libc;
use std::ptr::null;
use std::sync::atomic::{AtomicI32, Ordering};

// Futex-based mutex to ensure thread serialization
pub struct Futex {
    futex: AtomicI32,
}

impl Futex {
    pub fn new() -> Self {
        Self {
            futex: AtomicI32::new(1),
        }
    }

    #[inline]
    fn do_futex(addr: *const AtomicI32, op: i32, val: i32) {
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                addr,
                op,
                val,
                null::<libc::c_void>(),
                null::<libc::c_void>(),
                0,
            );
        }
    }

    #[inline]
    pub fn lock(&mut self) {
        if self.futex.fetch_sub(1, Ordering::Relaxed) != 1 {
            self.futex.store(-1, Ordering::Relaxed);
            Futex::do_futex(&self.futex, libc::FUTEX_PRIVATE_FLAG | libc::FUTEX_WAIT, -1);
        }
    }

    #[inline]
    pub fn unlock(&mut self) {
        if self.futex.fetch_add(1, Ordering::Relaxed) != 0 {
            self.futex.store(1, Ordering::Relaxed);
            Futex::do_futex(&self.futex, libc::FUTEX_PRIVATE_FLAG | libc::FUTEX_WAKE, 1);
        }
    }
}
