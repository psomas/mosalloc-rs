use std::env;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use clap::Parser;

use mosalloc::utils::argparse::{default_node, parse_file_path, parse_size};
use mosalloc::utils::htlb::*;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Cli {
    #[clap(short, long, action, help = "dryrun, don't reserve and use hugepages")]
    dryrun: bool,

    #[clap(short, long, action, help = "analyze the pool sizes")]
    analyze: bool,

    #[clap(short, long, value_parser = parse_file_path, help = "mosalloc library path (default: ./libmosalloc.so)")]
    lib: Option<String>,

    #[clap(long, value_parser = parse_file_path, help = "Brk and anon (mmap) pool intervals configuration (CSV)")]
    config: String,

    #[clap(long, value_parser = parse_size, default_value_t = 1 << 30, help = "Fole Pool size")]
    file_pool_size: usize,

    #[clap(long, value_parser = parse_size, default_value_t = 1 << 20, help = "Anon FFA size")]
    anon_ffa_size: usize,

    #[clap(long, value_parser = parse_size, default_value_t = 1 << 10, help = "File FFA size")]
    file_ffa_size: usize,

    #[clap(value_parser, help = "Binary to run")]
    program: String,

    #[clap(value_parser, help = "Program arguments")]
    args: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    let path = Path::new(&cli.config);

    let mmap = Pool::from_csv(AllocType::ANON, &path);
    let brk = Pool::from_csv(AllocType::BRK, &path);

    let req = supported_htlb_sizes()
        .iter()
        .map(|&x| mmap.nrpages(x) + brk.nrpages(x))
        .collect::<Vec<usize>>();

    let node = default_node();

    print_htlb_status_node(node);

    disable_thp(true);
    enable_overcommit(true);

    let htlb_req = HTLBReq { node, req };
    if !cli.dryrun {
        htlb_req.reserve_pages().unwrap();
    }

    print_htlb_status_node(node);

    MosallocConfig {
        pool_config: cli.config,
        file_pool_size: cli.file_pool_size,
        anon_ffa_size: cli.anon_ffa_size,
        file_ffa_size: cli.file_ffa_size,
        analyze_regions: cli.analyze,
        dryrun: cli.dryrun,
    }
    .save();

    let preload = cli.lib.unwrap_or("./libmosalloc.so".to_string());
    env::set_var(
        "LD_PRELOAD",
        format!(
            "{}:{}",
            preload,
            env::var("LD_PRELOAD").unwrap_or("".to_string())
        ),
    );
    println!("{}", Command::new(cli.program).args(cli.args).exec());
}
