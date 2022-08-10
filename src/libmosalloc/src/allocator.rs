use std::hint::black_box;
use std::path::Path;
use std::ptr::null;

use libc;

use crate::hooks;
use crate::region::*;

use mosalloc::utils::htlb::{AllocType, MosallocConfig, Pool};

const CHUNK: usize = 64;

#[derive(Debug)]
pub struct Allocator {
    heap: Region,
    anon_region: Region,
    file_region: Region,
    analyze: bool,
    dryrun: bool,

    drained: bool,
}

impl Allocator {
    pub fn new() -> Self {
        let config = MosallocConfig::load();

        let mut heap = Region::new(
            Pool::from_csv(AllocType::BRK, Path::new(&config.pool_config)),
            AllocType::BRK,
            1,
        );

        let mut anon_region = Region::new(
            Pool::from_csv(AllocType::ANON, Path::new(&config.pool_config)),
            AllocType::ANON,
            config.anon_ffa_size,
        );

        let mut file_region = Region::new(
            Pool::new_file_pool(config.file_pool_size),
            AllocType::FILE,
            config.file_ffa_size,
        );

        let initial_brk = hooks::libc_sbrk(0) as usize;

        heap.init(initial_brk);
        // move the program break to the start of the mosalloc managed heap
        assert!(hooks::libc_brk(heap.start as *mut libc::c_void) != -1);

        anon_region.init(heap.max);
        file_region.init(anon_region.max);

        Self {
            heap,
            anon_region,
            file_region,
            analyze: config.analyze_regions,
            dryrun: config.dryrun,
            drained: false,
        }
    }

    pub unsafe fn drain(&mut self) {
        while black_box(libc::malloc(CHUNK)) as *const u8 != null() {}
        *libc::__errno_location() = 0;
        self.drained = true;
    }

    // brk helper for sbrk and brk
    #[inline]
    unsafe fn do_brk(&mut self, addr: Option<usize>, incr: Option<isize>) -> usize {
        if !self.drained {
            *libc::__errno_location() = libc::ENOMEM;
            return usize::MAX;
        }

        self.heap.lock();

        let oldbrk = self.heap.end;
        let newbrk = addr.unwrap_or(oldbrk.checked_add_signed(incr.unwrap()).unwrap());

        // make sure brk doesn't exceed the mosalloc-managed heap
        if !self.heap.contains(newbrk) {
            self.heap.unlock();
            *libc::__errno_location() = libc::ENOMEM;
            usize::MAX
        } else {
            if newbrk > oldbrk {
                let prot = libc::PROT_READ | libc::PROT_WRITE;
                let flags = libc::MAP_ANONYMOUS | libc::MAP_PRIVATE;
                let len = newbrk - oldbrk;

                self.heap.alloc_range(oldbrk, len, prot, flags, self.dryrun);
            } else if newbrk < oldbrk {
                self.heap.free_range(newbrk, oldbrk - newbrk);
            }
            self.heap.unlock();
            oldbrk
        }
    }

    pub unsafe fn brk(&mut self, addr: usize) -> i32 {
        println!("brk 0x{:x}", addr);
        if self.do_brk(Some(addr), None) == usize::MAX {
            -1
        } else {
            0
        }
    }

    pub unsafe fn sbrk(&mut self, incr: isize) -> usize {
        println!("sbrk {}", incr);
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

    pub unsafe fn mmap(
        &mut self,
        addr: usize,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> usize {
        println!("mmap 0x{:x}, len: {}, fd: {}", addr, len, fd);

        // forward mmaps outside mosalloc regions to libc
        if addr >= self.file_region.max {
            return hooks::libc_mmap(addr as *mut libc::c_void, len, prot, flags, fd, offset)
                as usize;
        }

        let dryrun = self.dryrun;
        let drained = self.drained;
        let region = self.region_from_fd(fd);

        // make sure the mmap doesn't span regions
        assert!(addr == 0 || addr + len <= region.max);

        if !drained && region.alloc_type == AllocType::ANON {
            *libc::__errno_location() = libc::ENOMEM;
            return libc::MAP_FAILED as usize;
        }

        region.lock();
        let addr = region.alloc_range(addr, len, prot, flags, dryrun);
        region.unlock();

        if region.alloc_type == AllocType::FILE {
            assert!(
                hooks::libc_mmap(
                    addr as *mut libc::c_void,
                    len,
                    prot,
                    libc::MAP_FIXED_NOREPLACE | flags,
                    fd,
                    offset,
                ) != libc::MAP_FAILED
            );
        }

        addr
    }

    pub fn munmap(&mut self, addr: usize, len: usize) -> i32 {
        println!("munmap 0x{:x} {}", addr, len);
        // forward munmaps outside mosalloc regions to libc
        if addr >= self.file_region.max {
            return hooks::libc_munmap(addr as *mut libc::c_void, len);
        }

        let region = self.region_from_addr(addr).unwrap();

        // make sure the munmap doesn't span regions
        assert!(addr + len <= region.max);

        region.lock();
        region.free_range(addr, len);
        region.unlock();

        if region.alloc_type == AllocType::FILE {
            hooks::libc_munmap(addr as *mut libc::c_void, len)
        } else {
            0
        }
    }

    pub fn mprotect(&mut self, addr: usize, len: usize, prot: i32) -> i32 {
        println!("mprotect 0x{:x} {} {}", addr, len, prot);
        // forward mprotect outside mosalloc mem regions to libc
        if addr >= self.file_region.max
            || self.region_from_addr(addr).unwrap().alloc_type == AllocType::FILE
        {
            hooks::libc_munmap(addr as *mut libc::c_void, len)
        } else {
            // ignore mprotect for heap + anon regions for now
            0
        }
    }

    pub fn madvise(&mut self, addr: usize, len: usize, advice: i32) -> i32 {
        println!("madvise 0x{:x} {} {}", addr, len, advice);

        // forward madvise outside mosalloc mem regions to libc
        if addr >= self.file_region.max
            || self.region_from_addr(addr).unwrap().alloc_type == AllocType::FILE
        {
            hooks::libc_munmap(addr as *mut libc::c_void, len)
        } else {
            // ignore mprotect for heap + anon regions for now
            0
        }
    }

    pub fn mremap(
        &mut self,
        old_address: usize,
        old_size: usize,
        new_size: usize,
        flags: i32,
        new_address: usize,
    ) -> usize {
        println!(
            "mremap 0x{:x} {} {} {:x}",
            old_address, old_size, new_size, new_address
        );
        // forward mremaps outside mosalloc regions to libc
        if old_address >= self.file_region.max {
            return hooks::libc_mremap(
                old_address as *mut libc::c_void,
                old_size,
                new_size,
                flags,
                new_size as *mut libc::c_void,
            ) as usize;
        }

        let old_type = self.region_from_addr(old_address).unwrap().alloc_type;
        let new_type = self.region_from_addr(old_address).unwrap().alloc_type;

        assert_eq!(old_type, new_type);

        old_address
    }
}
