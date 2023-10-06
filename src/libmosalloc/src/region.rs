use libc;
use std::ops::Range;

use mosalloc::utils::htlb::{AllocType, Pool, PAGE_SIZE};
use mosalloc::utils::misc::{align_down, align_up};
use mosalloc::pr_dbg;

use crate::lock::Lock;
use crate::preload_hooks;

// struct for heap, anon and file mosalloc regions
#[derive(Debug)]
pub struct Region {
    // region type
    pub alloc_type: AllocType,

    // the htlb intervals pool
    pool: Pool,

    pub start: usize,
    pub end: usize,
    pub max: usize,

    pub max_pgsz: usize,
    pub len: usize,

    free_map: Vec<Range<usize>>,

    lock: Lock,
}

impl Region {
    pub fn new(pool: Pool, alloc_type: AllocType, len: usize) -> Self {
        let free_map = Vec::with_capacity(len);

        let (max_pgsz, len) = pool.intervals.iter().fold((0, 0), |(pgsz, end), x| {
            (x.pagesz.max(pgsz), x.end.max(end))
        });

        Self {
            pool,
            alloc_type,
            start: 0,
            end: 0,
            max: 0,
            max_pgsz,
            len,
            free_map,
            lock: Lock::new(true),
        }
    }

    pub fn init(&mut self, start: usize) {
        self.start = start;
        self.end = self.start;
        self.max = self.start + self.len;

        self.free_map.push(self.start..self.max);
    }

    #[inline]
    fn get_addr_pagesz(&self, addr: usize) -> usize {
        let offset = addr - self.start;

        self.pool
            .intervals
            .iter()
            .find_map(|x| {
                if x.start <= offset && x.end >= offset {
                    Some(x.pagesz)
                } else {
                    None
                }
            })
            .unwrap_or(PAGE_SIZE)
    }

    // allocate memory for the given addr based on the pool config
    #[inline]
    fn alloc(&self, addr: usize, pagesz: usize, prot: i32, flags: i32, dryrun: bool) {
        let mut hflags = flags | libc::MAP_FIXED_NOREPLACE;
        if pagesz > PAGE_SIZE && !dryrun {
            hflags |= libc::MAP_HUGETLB | (pagesz.trailing_zeros() as i32) << libc::MAP_HUGE_SHIFT;
        }

        let ret = preload_hooks::libc_mmap(
            align_down(addr, pagesz) as *mut libc::c_void,
            pagesz,
            prot | libc::PROT_READ | libc::PROT_WRITE,
            hflags,
            -1,
            0,
        );

        if ret == libc::MAP_FAILED {
            // this can happen if we've already mapped this interval
            unsafe {
                assert_eq!(*libc::__errno_location(), libc::EEXIST);
            }
        }
    }

    // allocate memory for the [start, end] range
    pub fn alloc_range(
        &mut self,
        addr: usize,
        len: usize,
        prot: i32,
        flags: i32,
        dryrun: bool,
    ) -> usize {
        let len = align_up(len, PAGE_SIZE);
        let mut start = self.del_range_from_freemap(addr, len);
        if start == usize::MAX {
            if (flags & libc::MAP_FIXED_NOREPLACE) != 0 {
                // this will trigger an EEXIST for FIXED_NORPLACE
                return start;
            } else if (flags & libc::MAP_FIXED) != 0 {
                // for MAP_FIXED, make sure that the whole requested range has been previously allocated
                assert!(self
                    .free_map
                    .iter()
                    .all(|x| !x.contains(&start) && !x.contains(&(start + len))));
                return addr;
            } else {
                // ignore the address hint for non FIXED requests
                start = self.del_range_from_freemap(0, len);
                if start == usize::MAX {
                    return start;
                }
            }
        }
        let end = start + len;

        if end > self.end {
            self.end = end;
        }

        // for file mapping, we don't need to allocate memory
        if self.alloc_type == AllocType::FILE {
            return start;
        }

        let mut cur = start;
        while cur < end {
            let pagesz = self.get_addr_pagesz(cur);
            cur = align_down(cur, pagesz);
            self.alloc(cur, pagesz, prot, flags, dryrun);
            cur += pagesz;
        }

        start
    }

    pub fn free_range(&mut self, start: usize, len: usize) {
        let len = align_up(len, PAGE_SIZE);
        self.add_range_to_freemap(start, len);
        if self.end == start + len {
            self.end = if let Some(r) = self.free_map.iter().last() {
                r.start
            } else {
                self.start
            };
        }
    }

    fn del_range_from_freemap(&mut self, start: usize, len: usize) -> usize {
        pr_dbg!("{:x} {} {:?}", start, len, self.free_map);
        let ridx = self
            .free_map
            .iter()
            .position(|x| (start == 0 || x.contains(&start)) && (x.len() - start) >= len);

        if ridx.is_none() {
            return usize::MAX;
        }

        let ridx = ridx.unwrap();

        let range_start = self.free_map[ridx].start;

        for r in self.free_map.iter() {
            pr_dbg!("{:x} - {:x}", r.start, r.end);
        }
        pr_dbg!(
            "del_range: start: {:x} range_start: {:x}",
            start, range_start
        );

        // remove the range if it's wholly allocated
        if self.free_map[ridx].len() == len {
            self.free_map.remove(ridx);
        } else if start == 0 || start == self.free_map[ridx].start {
            self.free_map[ridx].start += len;
        } else {
            let new_range = (start + len)..self.free_map[ridx].end;
            self.free_map[ridx].end = start;
            self.free_map.insert(ridx + 1, new_range);
        }

        for r in self.free_map.iter() {
            pr_dbg!("{:x} - {:x}", r.start, r.end);
        }
        pr_dbg!(
            "del_range: start: {:x} range_start: {:x}",
            start, range_start
        );
        if start == 0 {
            range_start
        } else {
            start
        }
    }

    fn add_range_to_freemap(&mut self, start: usize, len: usize) {
        pr_dbg!("{:x} {} {:?}", start, len, self.free_map);
        let end = start + len;

        let mut left = false;
        let mut right = false;

        // just add the range in the free map if empty
        if self.free_map.is_empty() {
            self.free_map.push(start..end);
            return;
        }

        // find where the range should go in the free map
        let idx = self
            .free_map
            .iter()
            .position(|x| x.start >= end)
            .unwrap_or(self.free_map.len());

        pr_dbg!("idx: {} {:x} {:x}", idx, start, end);

        // check if we can merge with a range to our left
        if idx > 0 && self.free_map[idx - 1].end == start {
            self.free_map[idx - 1].end = end;
            left = true;
        }

        // check if we can merge with a range to our left
        if idx < self.free_map.len() && self.free_map[idx].start == end {
            self.free_map[idx].start = start;
            right = true;
        }

        // if we merged with both ends, merge those together
        if left && right {
            self.free_map[idx - 1].end = self.free_map[idx].end;
            self.free_map.remove(idx);
        }

        if !left && !right {
            self.free_map.insert(idx, start..end);
        }
        for r in self.free_map.iter() {
            pr_dbg!("{:x} - {:x}", r.start, r.end);
        }
    }

    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.start && addr < self.max
    }

    #[inline]
    pub fn lock(&mut self) {
        self.lock.lock();
    }

    #[inline]
    pub fn unlock(&mut self) {
        self.lock.unlock();
    }
}
