use clap::{Parser, Subcommand};

use mosalloc::utils::argparse::{default_node, parse_htlb_req, parse_node};
use mosalloc::utils::htlb::{self, HTLBReq};
use mosalloc::utils::rangelist::{Id, RangeList};
use mosalloc::utils::sysfs_path::*;

#[derive(Parser)]
#[clap(author, version, about)]
struct Cli {
    #[clap(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Takes a HugeTLB allocation request in the form of i:j:k:... Each of the i, j, k, etc)
    /// correspond to the supported HugeTLB sizes, in ascending order, i.e. on a x86 machine which
    /// supports 2MB and 1GB huge pages, a request of '10:20' would reserve 10 2MB and 20 1GB huge
    /// pages. For machines which support intemediate sizes, e.g. Arm and RiscV, a valid request
    /// could be e.g. '20:10:0:1', for 20 64KB pages, 10 2MB pages, 0 1GB pages and 1 16GB page on
    /// an ARMv8 machine using a 4KB granule. Each size always correspond to the same index, and
    /// missing sizes are ignored, so that '20:10' is the same to '20:10:0:0'. Optionally, the NUMA
    /// node on which the allocation is supposed to happen is provided.
    Reserve {
        #[clap(short, long, value_parser = parse_node, default_value_t = default_node(), hide_default_value = true, help = "NUMA node (default: local)")]
        node: Id,
        #[clap(value_parser = parse_htlb_req, help = "Requested HTLB pages")]
        htlb_req: HTLBReq,
    },
    /// Prints the current configuration of the HugeTLB pages on the system and lists the supported
    /// sizes and a HugeTLB request template  for the reserve command.
    Status,
}

fn main() {
    let mut cli = Cli::parse();

    match &mut cli.cmd {
        Cmd::Status => {
            println!("{}\n", HTLBReq::req_fmt_help());
            RangeList::from_path(sysfs_path_online_nodes())
                .iter()
                .for_each(|n| htlb::print_htlb_status_node(n));
        }
        Cmd::Reserve { node, htlb_req } => {
            htlb_req.node = *node;

            htlb::print_htlb_status_node(*node);

            htlb::disable_thp(true);
            htlb::enable_overcommit(true);

            htlb_req.reserve_pages().unwrap();
        }
    }
}
