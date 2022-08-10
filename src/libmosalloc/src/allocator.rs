use std::path::Path;

use libc;

use crate::hooks;
use crate::region::*;

use mosalloc::utils::htlb::{AllocType, MosallocConfig, Pool};

pub struct Allocator {
    heap: Region,
    anon_region: Region,
    file_region: Region,
    analyze: bool,
    dryrun: bool,
}

impl Allocator {
    pub fn new() -> Self {
        let config = MosallocConfig::load();

        let mut heap = Region::new(
            Pool::from_csv(AllocType::BRK, Path::new(&config.pool_config)),
            None,
        );

        let mut anon_region = Region::new(
            Pool::from_csv(AllocType::ANON, Path::new(&config.pool_config)),
            Some(config.anon_ffa_size),
        );

        let mut file_region = Region::new(
            Pool::new_file_pool(config.file_pool_size),
            Some(config.file_ffa_size),
        );

        // allocations should be done by this point
        let initial_brk = hooks::libc_sbrk(0) as usize;

        heap.set_limits(initial_brk);
        // move the program break to the start of the mosalloc managed heap
        assert!(hooks::libc_brk(heap.start as *mut libc::c_void) != -1);

        anon_region.set_limits(heap.max);
        file_region.set_limits(anon_region.max);

        Self {
            heap,
            anon_region,
            file_region,
            analyze: config.analyze_regions,
            dryrun: config.dryrun,
        }
    }

    #[inline]
    fn get_brk(&self) -> usize {
        if self.heap.placement_map.is_empty() {
            self.heap.start
        } else {
            assert!(self.heap.placement_map.len() == 1);
            self.heap.placement_map[0].end
        }
    }

    // brk helper for sbrk and brk
    #[inline]
    fn do_brk(&mut self, addr: Option<usize>, incr: Option<isize>) -> usize {
        self.heap.lock();

        let oldbrk = self.get_brk();
        let newbrk = addr.unwrap_or(oldbrk.checked_add_signed(incr.unwrap()).unwrap());

        // make sure brk doesn't exceed the mosalloc-managed heap
        if !self.heap.contains(newbrk) {
            self.heap.unlock();
            usize::MAX
        } else {
            if newbrk > oldbrk {
                let prot = libc::PROT_READ | libc::PROT_WRITE;
                let flags = libc::MAP_ANONYMOUS | libc::MAP_PRIVATE;
                let len = newbrk - oldbrk;

                self.heap.add_range(oldbrk, len);
                self.heap.alloc_range(oldbrk, len, prot, flags, self.dryrun);
            } else if newbrk < oldbrk {
                self.heap.del_range(newbrk, oldbrk - newbrk);
            }
            self.heap.unlock();
            oldbrk
        }
    }

    pub fn brk(&mut self, addr: usize) -> i32 {
        if self.do_brk(Some(addr), None) == usize::MAX {
            -1
        } else {
            0
        }
    }

    pub fn sbrk(&mut self, incr: isize) -> usize {
        self.do_brk(None, Some(incr))
    }

    #[inline]
    fn region_from_addr(&mut self, addr: usize) -> Option<&mut Region> {
        if self.anon_region.contains(addr) {
            Some(&mut self.anon_region)
        } else if self.file_region.contains(addr) {
            Some(&mut self.file_region)
        } else {
            None
        }
    }

    #[inline]
    fn region_from_fd(&mut self, fd: i32) -> &mut Region {
        if fd == -1 {
            &mut self.anon_region
        } else {
            &mut self.file_region
        }
    }

    pub fn mmap(
        &mut self,
        addr: usize,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> usize {
        // forward mmaps outside mosalloc regions to libc
        if addr >= self.file_region.max {
            return hooks::libc_mmap(addr as *mut libc::c_void, len, prot, flags, fd, offset)
                as usize;
        }

        let dryrun = self.dryrun;
        let region = self.region_from_fd(fd);

        // make sure the mmap doesn't span regions
        assert!(addr == 0 || addr + len <= region.max);

        region.lock();
        let mut mmaped_at = region.add_range(addr, len);

        if fd == -1 {
            region.alloc_range(addr, len, prot, flags, dryrun);
        } else {
            mmaped_at = hooks::libc_mmap(
                mmaped_at as *mut libc::c_void,
                len,
                prot,
                libc::MAP_FIXED | flags,
                fd,
                offset,
            ) as usize;
        }
        region.unlock();

        mmaped_at
    }

    pub fn munmap(&mut self, addr: usize, len: usize) -> i32 {
        // forward munmaps outside mosalloc regions to libc
        if addr >= self.file_region.max {
            return hooks::libc_munmap(addr as *mut libc::c_void, len);
        }

        let region = self.region_from_addr(addr).unwrap();
        // make sure the munmap doesn't span regions
        assert!(addr + len <= region.max);

        region.lock();
        region.del_range(addr, len);
        region.unlock();

        0
    }
}
