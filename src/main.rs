#[macro_use]
extern crate log;

mod builder;
mod error;
mod linker;
mod peeker;

use builder::build_tree;
use clap::{ArgAction, Parser, Subcommand};
use error::Errors;
use linker::link_tree;
use log::LevelFilter;
use peeker::print_variables;
use std::env;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(short, long, env = "DOTFILES_PATH")]
    template_dir: Option<PathBuf>,

    #[arg(short, long)]
    build_dir: Option<PathBuf>,

    #[arg(short, long)]
    link_dir: Option<PathBuf>,

    #[arg(long = "variables")]
    variables_path: Option<PathBuf>,

    #[arg(short, action = ArgAction::Count)]
    verbosity: u8,

    flags: Vec<String>,

    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    Sync,
    Diff,
    Print,
}

#[derive(Debug)]
pub struct Config {
    template_dir: PathBuf,
    build_dir: PathBuf,
    link_dir: PathBuf,
    variables_path: PathBuf,
    flags: Vec<String>,
}

#[tokio::main]
async fn main() {
    match run().await {
        Ok(_) => {}
        Err(errors) => errors.log(),
    }
}

async fn run() -> Result<(), Errors> {
    let opt = Args::parse();

    let filter_level = match opt.verbosity {
        0 => LevelFilter::Warn,
        1 => LevelFilter::Info,
        2 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    pretty_env_logger::formatted_builder()
        .filter_level(filter_level)
        .init();

    let xdg_dirs = xdg::BaseDirectories::with_prefix("dotfiles").unwrap();

    let cfg = Config {
        template_dir: opt
            .template_dir
            .unwrap_or_else(|| xdg_dirs.create_config_directory("tree").expect("xdg")),
        build_dir: opt
            .build_dir
            .unwrap_or_else(|| xdg_dirs.create_cache_directory("").expect("xdg")),
        link_dir: opt
            .link_dir
            .unwrap_or_else(|| env::var("HOME").expect("$HOME").into()),
        variables_path: opt
            .variables_path
            .unwrap_or_else(|| xdg_dirs.get_config_file("variables.toml")),
        flags: opt.flags,
    };

    match opt.action {
        Action::Sync => {
            info!("building tree");
            build_tree(&cfg).await?;

            info!("linking tree");
            link_tree(&cfg).await?;
        }
        Action::Diff => {
            info!("building tree");
            build_tree(&cfg).await?;

            info!("checking differences between current state and dotfiles");
            todo!("not implemented");
        }
        Action::Print => {
            info!("scanning tree");
            print_variables(&cfg).await?;
        }
    }

    Ok(())
}
