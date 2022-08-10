use std::fs::File;
use std::hint::black_box;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::ptr::null;

use libc;

use crate::internal_allocator::InternalAllocator;
use crate::preload_hooks;
use crate::region::*;

use mosalloc::utils::htlb::{AllocType, MosallocConfig, Pool};
use mosalloc::utils::misc::align_up;

const CHUNK: usize = 64;
const NONSTD_FLAGS: i32 =
    libc::MAP_SHARED | libc::MAP_SHARED_VALIDATE | libc::MAP_GROWSDOWN | libc::MAP_HUGETLB;

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
    pub fn new(config: MosallocConfig, drained: bool) -> Self {
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

        let initial_brk = align_up(preload_hooks::libc_sbrk(0) as usize, heap.max_pgsz);

        // TODO: split this to a separate function
        let maps = BufReader::new(File::open("/proc/self/maps").unwrap());

        let mut last = 0;
        let mut upper = initial_brk;
        let mut region = &mut heap;

        'outer: for line in maps.lines() {
            let line = line.unwrap();
            let mapping: Vec<&str> = line.split_whitespace().collect();

            let addr_range: Vec<usize> = mapping[0]
                .splitn(2, '-')
                .map(|x| usize::from_str_radix(x, 16).unwrap())
                .collect();

            if last < upper {
                last = align_up(addr_range[1], region.max_pgsz);
                continue;
            }

            // found gap for the region
            while last >= upper && last < addr_range[0] && addr_range[0] - last >= region.len {
                region.init(last);
                upper = region.max;

                match region.alloc_type {
                    AllocType::BRK => {
                        // move the program break to the start of the mosalloc managed heap
                        assert!(preload_hooks::libc_brk(heap.start as *mut libc::c_void) != -1);
                        println!("brk {:x}", last);

                        region = &mut anon_region;
                        upper = align_up(upper, region.max_pgsz);
                        last = upper;
                    }
                    AllocType::ANON => {
                        println!("mmap {:x}", last);
                        region = &mut file_region;
                        upper = align_up(upper, region.max_pgsz);
                        last = upper;
                    }
                    AllocType::FILE => {
                        println!("file {:x}", last);
                        break 'outer;
                    }
                }
            }

            if region.alloc_type == AllocType::BRK || mapping[mapping.len() - 1].trim() == "[stack]"
            {
                // no space
                panic!();
            }
        }

        // FIXME: workaround to initialize the hooks
        preload_hooks::libc_mmap(usize::MAX as *mut libc::c_void, 0, 0, 0, -1, 0);
        preload_hooks::libc_munmap(usize::MAX as *mut libc::c_void, 0);
        preload_hooks::libc_mprotect(usize::MAX as *mut libc::c_void, 0, 0);
        preload_hooks::libc_madvise(usize::MAX as *mut libc::c_void, 0, 0);
        preload_hooks::libc_mremap(
            usize::MAX as *mut libc::c_void,
            0,
            0,
            0,
            usize::MAX as *mut libc::c_void,
        );

        Self {
            heap,
            anon_region,
            file_region,
            analyze: config.analyze_regions,
            dryrun: config.dryrun,
            drained,
        }
    }

    pub unsafe fn drain(&mut self) {
        while black_box(libc::malloc(CHUNK)) as *const u8 != null() {}
        *libc::__errno_location() = 0;
        self.drained = true;
        InternalAllocator::print_stats();
    }

    // brk helper for sbrk and brk
    pub unsafe fn do_brk(&mut self, addr: Option<usize>, incr: Option<isize>) -> usize {
        if !self.drained {
            *libc::__errno_location() = libc::ENOMEM;
            return usize::MAX;
        }

        self.heap.lock();

        let oldbrk = self.heap.end;
        let newbrk = addr.unwrap_or_else(|| oldbrk.checked_add_signed(incr.unwrap()).unwrap());

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
        // FIXME: make sure that we don't mess with the mosalloc-managed heap
        assert!(!self.heap.contains(addr));

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

    #[inline]
    fn region_from_req(&mut self, addr: usize, fd: i32) -> Option<&mut Region> {
        if addr == 0 {
            Some(self.region_from_fd(fd))
        } else {
            // FIXME: there's a corner case where we might get a request for an address of e.g. the
            // file region but without an fd, just allocate it as requested atm
            self.region_from_addr(addr)
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

        let dryrun = self.dryrun;
        let drained = self.drained;

        let region = self.region_from_req(addr, fd);

        // forward mmaps outside mosalloc regions and non-standard anon private requests to libc
        if region.is_none() {
            return preload_hooks::libc_mmap(addr as *mut libc::c_void, len, prot, flags, fd, offset)
                as usize;
        }

        let region = region.unwrap();

        if !drained && region.alloc_type == AllocType::ANON {
            *libc::__errno_location() = libc::ENOMEM;
            return libc::MAP_FAILED as usize;
        }

        // use libc for 'non-std' anon mapping (i.e. shared mappings, explicit hugetlb requests, stack mappings)
        if (region.alloc_type == AllocType::ANON) && ((flags & NONSTD_FLAGS) != 0) {
            return preload_hooks::libc_mmap(addr as *mut libc::c_void, len, prot, flags, fd, offset)
                as usize;
        }

        // make sure the mmap doesn't span regions
        assert!(addr == 0 || addr + len <= region.max);

        region.lock();
        let addr = region.alloc_range(addr, len, prot, flags, dryrun);
        region.unlock();

        if addr == usize::MAX {
            if (flags & libc::MAP_FIXED_NOREPLACE) != 0 {
                // for MAP_FIXED_NOREPLACE, return EEXIST if we cannot allocate the requested addr
                *libc::__errno_location() = libc::EEXIST;
            } else {
                // for the rest, just return ENOMEM
                *libc::__errno_location() = libc::ENOMEM;
            }
            return libc::MAP_FAILED as usize;
        }

        if region.alloc_type == AllocType::FILE {
            return preload_hooks::libc_mmap(addr as *mut libc::c_void, len, prot, flags, fd, offset)
                as usize;
        }
        addr
    }

    pub fn munmap(&mut self, addr: usize, len: usize) -> i32 {
        println!("munmap 0x{:x} {}", addr, len);

        // forward munmaps outside mosalloc regions to libc
        let region = self.region_from_addr(addr);
        if region.is_none() {
            return preload_hooks::libc_munmap(addr as *mut libc::c_void, len);
        }

        let region = region.unwrap();

        // make sure the munmap doesn't span regions
        assert!(addr + len <= region.max);

        region.lock();
        region.free_range(addr, len);
        region.unlock();

        if region.alloc_type == AllocType::FILE {
            preload_hooks::libc_munmap(addr as *mut libc::c_void, len)
        } else {
            0
        }
    }

    pub fn mprotect(&mut self, addr: usize, len: usize, prot: i32) -> i32 {
        println!("mprotect 0x{:x} {} {}", addr, len, prot);
        // forward mprotect outside mosalloc mem regions to libc
        let region = self.region_from_addr(addr);
        if region.is_none() || region.unwrap().alloc_type == AllocType::FILE {
            preload_hooks::libc_mprotect(addr as *mut libc::c_void, len, prot)
        } else {
            // ignore mprotect for heap + anon regions for now
            0
        }
    }

    pub fn madvise(&mut self, addr: usize, len: usize, advice: i32) -> i32 {
        println!("madvise 0x{:x} {} {}", addr, len, advice);

        // forward madvise outside mosalloc mem regions to libc
        let region = self.region_from_addr(addr);
        if region.is_none() || region.unwrap().alloc_type == AllocType::FILE {
            preload_hooks::libc_madvise(addr as *mut libc::c_void, len, advice)
        } else {
            // ignore madvise for heap + anon regions for now
            0
        }
    }

    pub unsafe fn mremap(
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

        let dryrun = self.dryrun;

        // forward mremaps outside mosalloc regions to libc
        let region = self.region_from_addr(old_address);
        if region.is_none() {
            return preload_hooks::libc_mremap(
                old_address as *mut libc::c_void,
                old_size,
                new_size,
                flags,
                new_address as *mut libc::c_void,
            ) as usize;
        }

        let region = region.unwrap();
        region.lock();

        // make the new mapping belongs in the same region
        assert!(((flags & libc::MREMAP_FIXED) != 0) && ((new_address + new_size) <= region.max));

        let mut req_addr = new_address;

        // non-fixed remap
        if flags & libc::MREMAP_FIXED == 0 {
            // FIXME: argument validation
            if old_size >= new_size {
                // we can always in-place shrink
                region.free_range(old_address + new_size, old_size - new_size);
                return old_address;
            } else {
                // for expansions, we need to check if there's space
                let addr = region.alloc_range(
                    old_address + old_size,
                    new_size - old_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
                    dryrun,
                );

                // return if we were able to expand the range, else continue to the fixed path
                if addr == old_address + old_size {
                    return old_address;
                }
                assert_eq!(addr, usize::MAX);

                if flags & libc::MREMAP_MAYMOVE == 0 {
                    *libc::__errno_location() = libc::ENOMEM;
                    return libc::MAP_FAILED as usize;
                }
                req_addr = 0;
            }
        }

        let addr = region.alloc_range(
            req_addr,
            new_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
            dryrun,
        );

        region.unlock();

        // for MAP_FIXED, return error if we cannot allocate the requested addr
        if (req_addr != addr) && (flags & libc::MREMAP_FIXED != 0) {
            // FIXME: which error code makes sense for MAP_FIXED?
            *libc::__errno_location() = libc::EEXIST;
            return libc::MAP_FAILED as usize;
        }

        new_address
    }
}
