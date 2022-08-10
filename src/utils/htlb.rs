use csv;
use serde::Deserialize;
use std::convert::From;
use std::env;
use std::fs;
use std::path::Path;

use super::misc::{size_from_str, size_to_str};
use super::rangelist::Id;
use super::sysfs_path::*;

pub const PAGE_SIZE: usize = 1 << 12;

// list of the system-supported HTLB sizes
pub fn supported_htlb_sizes() -> Vec<usize> {
    let mut sizes = fs::read_dir(sysfs_path_htlb_base())
        .unwrap()
        .map(|x| {
            x.unwrap()
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .splitn(2, '-')
                .skip(1)
                .next()
                .unwrap()
                .strip_suffix("kB")
                .unwrap()
                .parse::<usize>()
                .unwrap()
                << 10
        })
        .collect::<Vec<usize>>();

    sizes.sort();
    return sizes;
}

// helper to disable THP
pub fn disable_thp(readonly: bool) {
    if !readonly {
        fs::write(sysfs_path_thp_enabled(), "never").unwrap();
    }
    print!(
        "thp: {}",
        fs::read_to_string(sysfs_path_thp_enabled()).unwrap()
    );
}

// helper to enable overcommit
pub fn enable_overcommit(readonly: bool) {
    if !readonly {
        fs::write(sysfs_path_overcommit(), "1").unwrap();
    }
    print!(
        "overcommit: {}",
        fs::read_to_string(sysfs_path_overcommit()).unwrap()
    );
}

// helper to set (allocate / free) HTLB pages for a given NUMA node
pub fn set_htlb_pages_node(node: Id, sz: usize, nr: usize) -> Result<(), String> {
    let sizes = supported_htlb_sizes();
    if !sizes.contains(&sz) {
        Err(format!("invalid htlb size {}", sz))
    } else {
        Ok(fs::write(
            sysfs_path_htlb(node, sz >> 10, "nr_hugepages"),
            format!("{}", nr),
        )
        .unwrap())
    }
}

// helper to get the allocated HTLB pages for a given NUMA node
pub fn get_htlb_pages_node(node: Id, sz: usize) -> Result<usize, String> {
    let sizes = supported_htlb_sizes();
    if !sizes.contains(&sz) {
        Err(format!("invalid htlb size {}", sz))
    } else {
        Ok(
            fs::read_to_string(sysfs_path_htlb(node, sz >> 10, "nr_hugepages"))
                .unwrap()
                .trim()
                .parse::<usize>()
                .unwrap(),
        )
    }
}

// prints the reserved HTLB pages for a given NUMA node
pub fn print_htlb_status_node(node: Id) {
    println!("HugeTLB status for node {}", node);
    let sizes = supported_htlb_sizes();
    for &size in sizes.iter() {
        println!(
            "# of {} pages (node {}) == {}",
            size_to_str(size),
            node,
            get_htlb_pages_node(node, size).unwrap()
        );
    }
    println!();
}

// request to reserve HTLB pages for a given Node
#[derive(Clone, Debug)]
pub struct HTLBReq {
    pub req: Vec<usize>,
    pub node: Id,
}

impl HTLBReq {
    // returns the format string of the request (e.g. i:j:k:l)
    fn req_fmt_str(sizes: &Vec<usize>) -> String {
        sizes
            .iter()
            .enumerate()
            .map(|(i, _)| format!("{}", ('i' as u8 + i as u8) as char))
            .collect::<Vec<String>>()
            .join(":")
    }

    // returns the help string for the request format
    pub fn req_fmt_help() -> String {
        let sizes = supported_htlb_sizes();

        sizes
            .iter()
            .enumerate()
            .map(|(i, &sz)| {
                let c = ('i' as u8 + i as u8) as char;
                format!("{} {} pages", c, size_to_str(sz))
            })
            .collect::<Vec<String>>()
            .join(", ")
            + " -> "
            + &HTLBReq::req_fmt_str(&sizes)
    }

    // reserves the pages specified in the request
    pub fn reserve_pages(&self) -> Result<(), String> {
        let sizes = supported_htlb_sizes();

        sizes
            .iter()
            .zip(self.req.iter())
            .filter(|(&sz, &req_sz)| req_sz > get_htlb_pages_node(self.node, sz).unwrap())
            .for_each(|(&sz, &req_sz)| set_htlb_pages_node(self.node, sz, req_sz).unwrap());

        sizes
            .iter()
            .zip(self.req.iter())
            .rev()
            .for_each(|(&sz, &req_sz)| set_htlb_pages_node(self.node, sz, req_sz).unwrap());

        let check = sizes
            .iter()
            .zip(self.req.iter())
            .any(|(&sz, &req_sz)| req_sz != get_htlb_pages_node(self.node, sz).unwrap());

        if check {
            return Ok(());
        } else {
            return Err("Couldn't allocate pages".to_string());
        }
    }
}

// deserialized CSV interval
#[derive(Debug, Deserialize)]
struct CSVRecord {
    #[serde(rename = "type")]
    region_type: String,
    page_size: String,
    start_offset: String,
    end_offset: String,
}

// HTLB interval
#[derive(Debug)]
pub struct Interval {
    pub pagesz: usize,
    pub start: usize,
    pub end: usize,
}

impl From<CSVRecord> for Interval {
    fn from(rec: CSVRecord) -> Self {
        let pagesz = size_from_str(&rec.page_size);

        assert!(supported_htlb_sizes().contains(&pagesz), "invalid size");

        let start = size_from_str(&rec.start_offset);
        let end = size_from_str(&rec.end_offset);

        // alignment checks
        assert!(start & (pagesz - 1) == 0);
        assert!(end & (pagesz - 1) == 0);
        assert!(start != end);

        Interval { pagesz, start, end }
    }
}

// allocation types for HTLB pools
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum AllocType {
    BRK,
    ANON,
    FILE,
}

impl AllocType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AllocType::BRK => "brk",
            AllocType::ANON => "mmap",
            AllocType::FILE => "file",
        }
    }
}

// libmosalloc config
pub struct MosallocConfig {
    pub pool_config: String,

    pub anon_ffa_size: usize,
    pub file_ffa_size: usize,
    pub file_pool_size: usize,

    pub analyze_regions: bool,
    pub dryrun: bool,
}

impl MosallocConfig {
    // Get the config from env and create a new config
    pub fn load() -> Self {
        let pool_config = env::var("HPC_CONFIG_FILE").unwrap();

        let anon_ffa_size = env::var("HPC_ANON_FFA_SIZE")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let file_ffa_size = env::var("HPC_FILE_FFA_SIZE")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let file_pool_size = env::var("HPC_FILE_POOL_SIZE")
            .unwrap()
            .parse::<usize>()
            .unwrap();

        let analyze_regions = env::var("HPC_ANALYZE_HPBRS")
            .unwrap()
            .parse::<bool>()
            .unwrap();

        let dryrun = env::var("HPC_DRYRUN").unwrap().parse::<bool>().unwrap();

        Self {
            pool_config,
            anon_ffa_size,
            file_ffa_size,
            file_pool_size,
            analyze_regions,
            dryrun,
        }
    }

    // Set the env according to the config
    pub fn save(&self) {
        env::set_var("HPC_ANON_FFA_SIZE", self.anon_ffa_size.to_string());
        env::set_var("HPC_FILE_FFA_SIZE", self.file_ffa_size.to_string());
        env::set_var("HPC_FILE_POOL_SIZE", self.file_pool_size.to_string());
        env::set_var("HPC_ANALYZE_HPBRS", self.analyze_regions.to_string());
        env::set_var("HPC_DRYRUN", self.dryrun.to_string());
        env::set_var("HPC_CONFIG_FILE", &self.pool_config);
    }
}

// HTLB intervals pool
#[derive(Debug)]
pub struct Pool {
    pub alloc_type: AllocType,
    pub intervals: Vec<Interval>,
}

impl Pool {
    // Create a new pseudo-htlb pool for file-mapped regions, page size is fixed at 4KB
    pub fn new_file_pool(sz: usize) -> Self {
        assert!(sz & PAGE_SIZE == 0);
        Pool {
            alloc_type: AllocType::FILE,
            intervals: vec![Interval {
                pagesz: PAGE_SIZE,
                start: 0,
                end: sz,
            }],
        }
    }

    // Create a new htlb pool from the intervals-holding CSV config
    pub fn from_csv(alloc_type: AllocType, config: &Path) -> Self {
        let mut intervals = csv::Reader::from_path(config)
            .unwrap()
            .deserialize()
            .filter_map(|x| {
                let rec: CSVRecord = x.unwrap();
                if rec.region_type == alloc_type.as_str() {
                    Some(Interval::from(rec))
                } else {
                    None
                }
            })
            .collect::<Vec<Interval>>();

        intervals.sort_by_key(|k| k.start);

        let prev = &intervals[0];
        for next in intervals.iter().skip(1) {
            assert!(prev.end <= next.start, "overlapping intervals");
        }

        Pool {
            alloc_type,
            intervals,
        }
    }

    // number of HTLB pages of a given size in the pool
    pub fn nrpages(&self, sz: usize) -> usize {
        self.intervals
            .iter()
            .filter_map(|x| {
                if x.pagesz == sz {
                    Some((x.end - x.start) / x.pagesz)
                } else {
                    None
                }
            })
            .sum()
    }

    // total size of the HTLB pages in the pool
    pub fn size(&self) -> usize {
        supported_htlb_sizes()
            .iter()
            .fold(0, |acc, &x| acc + self.nrpages(x) * x)
    }
}
