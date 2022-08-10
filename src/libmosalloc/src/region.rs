use libc;
use std::mem::size_of;
use std::ops::Range;

use mosalloc::utils::htlb::{Pool, PAGE_SIZE};
use mosalloc::utils::misc::{align_down, align_up};

use crate::futex::Futex;
use crate::hooks;

// struct for heap (brk) anon and file mosalloc regions
pub struct Region {
    // the htlb intervals pool
    pool: Pool,

    pub start: usize,
    pub max: usize,

    pub placement_map: Vec<Range<usize>>,
    alloc_map: Vec<Range<usize>>,

    futex: Futex,
}

impl Region {
    pub fn new(pool: Pool, cap: Option<usize>) -> Self {
        let placement_map =
            Vec::with_capacity(cap.unwrap_or(PAGE_SIZE) / size_of::<Range<usize>>());
        let alloc_map = Vec::with_capacity(cap.unwrap_or(PAGE_SIZE) / size_of::<Range<usize>>());

        Self {
            pool,
            start: 0,
            max: 0,
            placement_map,
            alloc_map,
            futex: Futex::new(),
        }
    }

    pub fn set_limits(&mut self, start: usize) {
        let max_pgsz = self
            .pool
            .intervals
            .iter()
            .max_by_key(|x| x.pagesz)
            .unwrap()
            .pagesz;

        self.start = align_up(start, max_pgsz);
        self.max = self.start
            + self
                .pool
                .intervals
                .iter()
                .max_by_key(|x| x.end)
                .unwrap()
                .end;
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
        let mut hflags = flags | libc::MAP_FIXED;
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

        assert!(ret != libc::MAP_FAILED);
    }

    #[inline]
    fn add_range_to(map: &mut Vec<Range<usize>>, start: usize, end: usize) {
        let left = map.iter().position(|x| x.contains(&(start - 1)));
        let right = map.iter().position(|x| x.contains(&end));

        if let Some(lidx) = left {
            if let Some(ridx) = right {
                map[lidx].end = map[ridx].end;
                map.swap_remove(ridx);
            } else {
                map[lidx].end = end;
            }
        } else if let Some(ridx) = right {
            map[ridx].start = start;
        } else {
            map.push(start..end);
        }
    }

    // allocate memory for the [start, end] range
    pub fn alloc_range(&mut self, start: usize, len: usize, prot: i32, flags: i32, dryrun: bool) {
        let end = start + len;
        let alloc_start = align_down(start, self.get_addr_pagesz(start));

        let mut cur = alloc_start;
        while end >= cur {
            let pagesz = self.get_addr_pagesz(cur);
            if !self.alloc_map.iter().any(|x| x.contains(&cur)) {
                self.alloc(cur, pagesz, prot, flags, dryrun);
            }
            cur += pagesz;
        }

        Self::add_range_to(&mut self.alloc_map, alloc_start, cur);
    }

    #[inline]
    fn place(&self, len: usize) -> usize {
        let mut start = 0;

        for range in self.placement_map.iter() {
            if range.start - start >= len {
                return start;
            }
            start = range.end;
        }

        assert!(self.max - start >= len);
        return start;
    }

    pub fn add_range(&mut self, start: usize, len: usize) -> usize {
        let addr = if start == 0 { self.place(len) } else { start };

        Self::add_range_to(&mut self.placement_map, addr, addr + len);

        addr
    }

    #[inline]
    fn del_range_from(map: &mut Vec<Range<usize>>, start: usize, end: usize) {
        let idx = map.iter().position(|x| x.contains(&start)).unwrap();

        if map[idx].start == start && map[idx].end == end {
            map.swap_remove(idx);
        } else if map[idx].start == start {
            map[idx].start = end;
        } else if map[idx].end == end {
            map[idx].end = start;
        } else {
            let tmp = map[idx].end;
            map[idx].end = start;
            map.push(end..tmp);
        }
    }

    pub fn del_range(&mut self, start: usize, len: usize) {
        Self::del_range_from(&mut self.placement_map, start, start + len);
    }

    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.start && addr < self.max
    }

    #[inline]
    pub fn lock(&mut self) {
        self.futex.lock();
    }

    #[inline]
    pub fn unlock(&mut self) {
        self.futex.unlock();
    }
}
