#[macro_use]
extern crate log;

mod builder;
mod diff;
mod error;
mod linker;
mod peeker;

use builder::build_tree;
use clap::{ArgAction, Parser, Subcommand};
use diff::calculate_diff;
use error::Errors;
use linker::link_tree;
use log::LevelFilter;
use peeker::print_variables;
use std::env;
use std::path::{Path, PathBuf};

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
    Diff {
        /// Also print what has changed in files if they exist both locally and in repo.
        #[arg(long, short)]
        line_changes: bool,
    },
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

impl Config {
    fn with_build_path(&self, build_dir: &Path) -> Self {
        Self {
            template_dir: self.template_dir.clone(),
            build_dir: build_dir.to_path_buf(),
            link_dir: self.link_dir.clone(),
            variables_path: self.variables_path.clone(),
            flags: self.flags.clone(),
        }
    }
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
        Action::Diff { line_changes } => {
            info!("checking diffs");
            calculate_diff(&cfg, line_changes).await?;
        }
        Action::Print => {
            info!("scanning tree");
            print_variables(&cfg).await?;
        }
    }

    Ok(())
}
