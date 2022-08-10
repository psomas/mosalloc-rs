use libc;
use std::ops::Range;

use mosalloc::utils::htlb::{AllocType, Pool, PAGE_SIZE};
use mosalloc::utils::misc::{align_down, align_up};

use crate::hooks;
use crate::lock::Lock;

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

    free_map: Vec<Range<usize>>,

    lock: Lock,
}

impl Region {
    pub fn new(pool: Pool, alloc_type: AllocType, len: usize) -> Self {
        let free_map = Vec::with_capacity(len);

        Self {
            pool,
            alloc_type,
            start: 0,
            end: 0,
            max: 0,
            free_map,
            lock: Lock::new(),
        }
    }

    pub fn init(&mut self, start: usize) {
        let (max_pgsz, end) = self.pool.intervals.iter().fold((0, 0), |(pgsz, end), x| {
            (x.pagesz.max(pgsz), x.end.max(end))
        });

        self.start = align_up(start, max_pgsz);
        self.end = self.start;
        self.max = self.start + end;

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

        let ret = hooks::libc_mmap(
            align_down(addr, pagesz) as *mut libc::c_void,
            pagesz,
            prot,
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
        start: usize,
        len: usize,
        prot: i32,
        flags: i32,
        dryrun: bool,
    ) -> usize {
        let start = self.del_range_from_freemap(start, len);
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
        let ridx = self
            .free_map
            .iter()
            .position(|x| x.len() >= len && (start == 0 || x.contains(&start)))
            .unwrap();

        let range_start = self.free_map[ridx].start;

        for r in self.free_map.iter() {
            println!("{:x} - {:x}", r.start, r.end);
        }
        println!(
            "del_range: start: {:x} range_start: {:x}",
            start, range_start
        );

        // remove the range if it's wholly allocated
        if self.free_map[ridx].len() == len {
            assert_eq!(self.free_map[ridx].start, start);
            self.free_map.remove(ridx);
        } else if start == 0 || start == self.free_map[ridx].start {
            self.free_map[ridx].start += len;
        } else {
            let new_range = (start + len)..self.free_map[ridx].end;
            self.free_map[ridx].end = start;
            self.free_map.insert(ridx + 1, new_range);
        }

        for r in self.free_map.iter() {
            println!("{:x} - {:x}", r.start, r.end);
        }
        println!(
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

        println!("idx: {} {:x} {:x}", idx, start, end);

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
            println!("{:x} - {:x}", r.start, r.end);
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
