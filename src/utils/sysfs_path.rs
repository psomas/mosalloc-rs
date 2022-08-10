use std::path::{Path, PathBuf};

use super::rangelist::Id;

const SYSFS_NODES: &str = "/sys/devices/system/node/";
const SYSFS_CPUS: &str = "/sys/devices/system/cpu/";
const SYSFS_HTLB: &str = "/sys/kernel/mm/hugepages/";
const SYSFS_THP_ENABLED: &str = "/sys/kernel/mm/transparent_hugepage/enabled";
const SYSFS_OVERCOMMIT: &str = "/proc/sys/vm/overcommit_memory";

pub fn sysfs_path_online_cpus() -> PathBuf {
    Path::new(SYSFS_CPUS).join("online")
}

pub fn sysfs_path_online_nodes() -> PathBuf {
    Path::new(SYSFS_NODES).join("online")
}

pub fn sysfs_path_node(n: Id) -> PathBuf {
    Path::new(SYSFS_NODES).join(format!("node{}", n))
}

pub fn sysfs_path_node_cpus(n: Id) -> PathBuf {
    sysfs_path_node(n).join("cpulist")
}

pub fn sysfs_path_htlb_base() -> PathBuf {
    PathBuf::from(SYSFS_HTLB)
}

pub fn sysfs_path_htlb(n: Id, sz: usize, leaf: &str) -> PathBuf {
    sysfs_path_node(n)
        .join("hugepages")
        .join(format!("hugepages-{}kB", sz))
        .join(leaf)
}

pub fn sysfs_path_thp_enabled() -> PathBuf {
    PathBuf::from(SYSFS_THP_ENABLED)
}

pub fn sysfs_path_overcommit() -> PathBuf {
    PathBuf::from(SYSFS_OVERCOMMIT)
}
