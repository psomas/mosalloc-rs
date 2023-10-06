use epoll;
use libseccomp::notify::*;
use libseccomp::*;
use std::sync::mpsc::sync_channel;
use std::thread;
use syscalls::Sysno;

use crate::allocator::Allocator;
use crate::preload_hooks::libc_brk;

use mosalloc::utils::htlb::MosallocConfig;
use mosalloc::pr_dbg;

const SYSCALLS: [&'static str; 6] = ["brk", "mmap", "munmap", "mprotect", "madvise", "mremap"];

// mosalloc allocator instance when seccomp hooks are used
static mut SECCOMP_MOSALLOC: Option<Allocator> = None;

pub unsafe fn seccomp_init(config: MosallocConfig) {
    let (fd_tx, fd_rx) = sync_channel::<i32>(0);
    let (stx, srx) = sync_channel::<bool>(0);

    thread::spawn(move || {
        let fd = fd_rx.recv().unwrap();

        SECCOMP_MOSALLOC = Some(Allocator::new(config, false));
        let mosalloc = SECCOMP_MOSALLOC.as_mut().unwrap();
        stx.send(true).unwrap();

        let pfd = epoll::create(false).unwrap();

        let event = epoll::Event::new(epoll::Events::EPOLLIN, 0);
        epoll::ctl(pfd, epoll::ControlOptions::EPOLL_CTL_ADD, fd, event).unwrap();

        let mut err;
        let mut ret;

        loop {
            epoll::wait(pfd, -1, &mut [event]).unwrap();
            let req = ScmpNotifReq::receive(fd).unwrap();
            pr_dbg!("got syscall {}", req.data.syscall);

            match req.data.syscall {
                brk if brk == Sysno::brk as i32 => {
                    let oldbrk = mosalloc.do_brk(Some(req.data.args[0] as usize), None);
                    ret = if oldbrk != usize::MAX {
                        req.data.args[0] as i64
                    } else {
                        libc_brk(0 as _) as _
                    };
                    err = 0;
                }
                mmap if mmap == Sysno::mmap as i32 => {
                    ret = mosalloc.mmap(
                        req.data.args[0] as usize,
                        req.data.args[1] as usize,
                        req.data.args[2] as i32,
                        req.data.args[3] as i32,
                        req.data.args[4] as i32,
                        req.data.args[5] as i64,
                    ) as i64;
                    err = if ret != libc::MAP_FAILED as i64 {
                        0
                    } else {
                        pr_dbg!("{}", *libc::__errno_location());
                        -*libc::__errno_location()
                    };
                }
                munmap if munmap == Sysno::munmap as i32 => {
                    ret = mosalloc.munmap(req.data.args[0] as usize, req.data.args[1] as usize)
                        as i64;
                    err = if ret == 0 as i64 {
                        0
                    } else {
                        -*libc::__errno_location()
                    };
                }
                mprotect if mprotect == Sysno::mprotect as i32 => {
                    ret = mosalloc.mprotect(
                        req.data.args[0] as usize,
                        req.data.args[1] as usize,
                        req.data.args[2] as i32,
                    ) as i64;
                    err = if ret == 0 as i64 {
                        0
                    } else {
                        *libc::__errno_location()
                    };
                }
                madvise if madvise == Sysno::madvise as i32 => {
                    ret = mosalloc.madvise(
                        req.data.args[0] as usize,
                        req.data.args[1] as usize,
                        req.data.args[2] as i32,
                    ) as i64;
                    err = if ret == 0 as i64 {
                        0
                    } else {
                        -*libc::__errno_location()
                    };
                }
                mremap if mremap == Sysno::mremap as i32 => {
                    ret = mosalloc.mremap(
                        req.data.args[0] as usize,
                        req.data.args[1] as usize,
                        req.data.args[2] as usize,
                        req.data.args[3] as i32,
                        req.data.args[4] as usize,
                    ) as i64;
                    err = if ret != libc::MAP_FAILED as i64 {
                        0
                    } else {
                        -*libc::__errno_location()
                    };
                }
                _ => {
                    panic!();
                }
            }

            pr_dbg!("ret: {:x}, err: {}", ret, err);
            let resp = ScmpNotifResp::new(req.id, ret, err, 0);
            resp.respond(fd).unwrap();
        }
    });

    let mut filter = ScmpFilterContext::new_filter(ScmpAction::Allow).unwrap();

    filter.add_arch(ScmpArch::Native).unwrap();

    for sc in SYSCALLS.iter() {
        // FIXME: add finer grained control for e.g. mmap ranges or fds
        filter
            .add_rule(ScmpAction::Notify, ScmpSyscall::from_name(sc).unwrap())
            .unwrap();
    }

    filter.load().unwrap();
    fd_tx.send(filter.get_notify_fd().unwrap()).unwrap();
    srx.recv().unwrap();

    // FIXME: do we need to drain?
    SECCOMP_MOSALLOC.as_mut().unwrap().drain();
}
