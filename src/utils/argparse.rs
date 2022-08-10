use nix::sched::{sched_getaffinity, CpuSet};
use nix::unistd::Pid;
use std::path::Path;

use super::htlb::{self, HTLBReq};
use super::misc::*;
use super::rangelist::{Id, RangeList};
use super::sysfs_path::*;

pub fn parse_file_path(s: &str) -> Result<String, String> {
    if Path::new(s).is_file() {
        Ok(s.to_owned())
    } else {
        Err(format!("invalid file {}", s))
    }
}

pub fn parse_size(s: &str) -> Result<usize, String> {
    Ok(size_from_str(s))
}

pub fn default_node() -> Id {
    let cpu_set = sched_getaffinity(Pid::from_raw(0)).unwrap();
    let cpus = RangeList::from_path(sysfs_path_online_cpus());
    let nodes = RangeList::from_path(sysfs_path_online_nodes());

    for cpu in 0..CpuSet::count() {
        if cpu_set.is_set(cpu).unwrap()
            && cpus.contains(cpu)
            && nodes
                .iter()
                .any(|n| RangeList::from_path(sysfs_path_node_cpus(n)).contains(cpu))
        {
            return cpu;
        }
    }

    panic!();
}

pub fn parse_node(s: &str) -> Result<Id, String> {
    let node = s
        .parse::<Id>()
        .map_err(|_| format!("`{}` isn't a number", s))?;
    if RangeList::from_path(sysfs_path_online_nodes()).contains(node) {
        Ok(node)
    } else {
        Err(format!("Invalid NUMA node {}", node))
    }
}

pub fn parse_htlb_req(s: &str) -> Result<HTLBReq, String> {
    let supported_sizes = htlb::supported_htlb_sizes();

    let req = s
        .trim()
        .splitn(supported_sizes.len(), ':')
        .map(|x| x.parse::<usize>());

    if req.clone().any(|x| x.is_err()) {
        Err(format!("Invalid HTLB request {}", s))
    } else {
        Ok(HTLBReq {
            req: req.map(|x| x.unwrap()).collect(),
            node: 0,
        })
    }
}
