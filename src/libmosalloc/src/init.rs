use ctor::ctor;

use mosalloc::utils::htlb::{HookType, MosallocConfig};

use crate::preload_hooks::preload_init;
use crate::seccomp_hooks::seccomp_init;

#[ctor]
unsafe fn activate_mosalloc() {
    let config = MosallocConfig::load();

    match config.hook {
        HookType::PRELOAD => {
            preload_init(config);
        }
        HookType::SECCOMP => {
            seccomp_init(config);
        }
    }
}
