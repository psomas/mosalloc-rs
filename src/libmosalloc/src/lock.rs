use std::hint;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

#[derive(Debug)]
pub struct Lock {
    lock: AtomicBool,
}

const LOOPS_PER_YIELD: u16 = 1000;

impl Lock {
    pub fn new(val: bool) -> Self {
        Self {
            lock: AtomicBool::new(val),
        }
    }

    #[inline]
    pub fn lock(&mut self) {
        let mut loops = 0;
        while self
            .lock
            .compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            loops += 1;
            if loops == LOOPS_PER_YIELD {
                loops = 0;
                thread::yield_now();
            }
            hint::spin_loop();
        }
    }

    #[inline]
    pub fn unlock(&mut self) {
        self.lock.store(true, Ordering::Release);
    }
}
