use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use colored::Colorize;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use similar::TextDiff;
use tempdir::TempDir;

use crate::{
    builder::build_tree,
    error::{ErrorLocation, Errors},
    Config,
};

/// Resolves a user supplied local path into a path relative to the link dir.
fn relative_filter(cfg: &Config, path: &Path) -> Result<PathBuf, Errors> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().with_location(path)?.join(path)
    };
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let link_dir = cfg
        .link_dir
        .canonicalize()
        .unwrap_or_else(|_| cfg.link_dir.clone());

    match absolute.strip_prefix(&link_dir) {
        Ok(rel) => Ok(rel.to_path_buf()),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{absolute:?} is not inside the link dir {link_dir:?}"),
        )
        .with_location(path)
        .into()),
    }
}

pub async fn calculate_diff(
    cfg: &Config,
    line_changes: bool,
    path_filter: Option<&Path>,
) -> Result<(), Errors> {
    let filter = path_filter.map(|p| relative_filter(cfg, p)).transpose()?;
    let temp_dir = TempDir::new("dofiles_diff_dir").with_location(&Path::new("/tmp"))?;

    let new_cfg = cfg.with_build_path(temp_dir.path());

    // Build the repository in a temporary directory such that templates are rendered properly etc.
    info!("Building tree in {temp_dir:?}");
    build_tree(&new_cfg).await?;

    // let diffs = diff_dir(temp_dir.path(), &cfg.link_dir, &PathBuf::new())?;
    let mut diffs = diff_dir_ignore(temp_dir.path(), &cfg.link_dir)?;

    if let Some(filter) = &filter {
        let keep = |p: &PathBuf| p.starts_with(filter);
        diffs.only_in_local.retain(keep);
        diffs.only_in_repo.retain(keep);
        diffs.in_both.retain(keep);
    }

    println!(
        "{}",
        format!(
            "Exists only locally:{}",
            if diffs.only_in_local.is_empty() {
                " None"
            } else {
                ""
            }
        )
        .bold()
    );
    for add in diffs.only_in_local.iter() {
        println!(" + {}", to_printable(&cfg.link_dir.join(add)).green());
    }

    println!(
        "{}",
        format!(
            "Exists only in repo:{}",
            if diffs.only_in_repo.is_empty() {
                " None"
            } else {
                ""
            }
        )
        .bold()
    );
    for miss in diffs.only_in_repo.iter() {
        println!(" - {}", to_printable(&cfg.template_dir.join(miss)).red());
    }

    println!(
        "{}",
        String::from("File exists in both but have been modified:").bold()
    );
    for maybe in diffs.in_both.iter() {
        let local = cfg.link_dir.join(&maybe);
        let repo = temp_dir.path().join(&maybe);

        if local.is_dir() {
            debug!(
                " dir exists in both:\n\t{}\n\t{}",
                to_printable(&local),
                to_printable(&repo)
            );
            continue;
        }

        if file_diff(&local, &repo)? {
            if line_changes {
                let file_name = format!("# {} #", to_printable(maybe));
                let border = "#".repeat(file_name.len());
                println!("{}", border.white().on_black());
                println!("{}", file_name.white().on_black());
                println!("{}", border.white().on_black());

                print_file_diffs(&local, &repo)?;
            } else {
                println!(
                    " file differs {} != {}",
                    to_printable(&local).bright_green(),
                    to_printable(&cfg.template_dir.join(&maybe)).bright_red()
                );
            }
        } else {
            debug!(
                " file matches:\n\t{}\n\t{}",
                to_printable(&local),
                to_printable(&repo)
            );
        }
    }

    Ok(())
}

struct Diffs {
    /// Set of relative paths that could only be found in the repo and not locally.
    only_in_repo: HashSet<PathBuf>,
    /// Set of relative paths that could only be found locally but not in the repo.
    only_in_local: HashSet<PathBuf>,
    /// Set of relative paths that were found in both.
    in_both: HashSet<PathBuf>,
}

const DOTFILES_IGNORE_NAME: &str = ".dotfilesignore";

fn diff_dir_ignore(repo_base: &Path, local_base: &Path) -> Result<Diffs, Errors> {
    let local_walker = WalkBuilder::new(local_base)
        .add_custom_ignore_filename(DOTFILES_IGNORE_NAME)
        .hidden(false)
        .build();

    let mut diffs = Diffs {
        only_in_repo: HashSet::new(),
        only_in_local: HashSet::new(),
        in_both: HashSet::new(),
    };

    for p in local_walker {
        let local_entry = p.with_location(local_base)?;
        let local_path = local_entry.path();
        debug!("Checking local path {local_path:?}");
        let relative_path = local_entry
            .path()
            .strip_prefix(local_base)
            .with_location(local_path)?;
        debug!("relative path: {relative_path:?}");
        let repo_path = repo_base.join(relative_path);

        let relative_path = relative_path.to_path_buf();

        if !repo_path.exists() {
            debug!("path only exists in local: {relative_path:?}");
            diffs.only_in_local.insert(relative_path);
        } else {
            if repo_path.is_dir() != local_path.is_dir() {
                debug!("Local path differs from repo path {relative_path:?}");
                diffs.only_in_local.insert(relative_path.clone());
                diffs.only_in_repo.insert(relative_path);
            } else {
                debug!("path exists in both: {relative_path:?}");
                diffs.in_both.insert(relative_path);
            }
        }
    }

    let repo_walker = WalkBuilder::new(repo_base)
        .add_custom_ignore_filename(DOTFILES_IGNORE_NAME)
        .hidden(false)
        .build();

    for p in repo_walker {
        let repo_entry = p.with_location(repo_base)?;
        let repo_path = repo_entry.path();
        debug!("Checking repo path {repo_path:?}");
        let relative_path = repo_entry
            .path()
            .strip_prefix(repo_base)
            .with_location(repo_path)?;
        debug!("relative path: {relative_path:?}");

        if diffs.only_in_repo.contains(relative_path) || diffs.in_both.contains(relative_path) {
            debug!("Aleady handled {relative_path:?}");
            continue;
        }

        // We've already checked if it exists in both during the local walkthrough.
        // If it exists in both but are of different types it should still be added to repo here.
        debug!("path exists only in repo {relative_path:?}");
        diffs.only_in_repo.insert(relative_path.to_path_buf());
    }

    Ok(diffs)
}

fn file_diff(first: &Path, second: &Path) -> Result<bool, Errors> {
    let (first_hash, first_size) = hash_file(first)?;
    let (second_hash, second_size) = hash_file(second)?;

    Ok(first_size != second_size || first_hash != second_hash)
}

#[inline(always)]
fn hash_file(path: &Path) -> Result<(Vec<u8>, u64), Errors> {
    let mut file = fs::File::open(path).with_location(path)?;
    let mut hasher = Sha256::new();
    let size = io::copy(&mut file, &mut hasher).with_location(path)?;
    let hash = hasher.finalize().to_vec();

    Ok((hash, size))
}

fn print_file_diffs(first: &Path, second: &Path) -> Result<(), Errors> {
    let Some(first_text) = get_file_text_or_print(first)? else {
        return Ok(());
    };

    let Some(second_text) = get_file_text_or_print(second)? else {
        return Ok(());
    };

    let diffs = TextDiff::from_lines(&first_text, &second_text);

    for diff in diffs.iter_all_changes() {
        match diff.tag() {
            similar::ChangeTag::Equal => {
                continue;
            }
            similar::ChangeTag::Delete => {
                let start = diff.old_index().expect("Old index to exist");
                for (i, line) in diff
                    .as_str()
                    .expect("Diff to be valid utf-8")
                    .lines()
                    .enumerate()
                {
                    println!("{}", format!(" - {}: {line}", start + i).bright_red());
                }
            }
            similar::ChangeTag::Insert => {
                let start = diff.new_index().expect("new index to exist");
                for (i, line) in diff
                    .as_str()
                    .expect("Diff to be valid utf-8")
                    .lines()
                    .enumerate()
                {
                    println!("{}", format!(" + {}: {line}", start + i).bright_green());
                }
            }
        }
    }

    Ok(())
}

fn get_file_text_or_print(path: &Path) -> Result<Option<String>, Errors> {
    let file = fs::File::open(path).with_location(path)?;

    let bytes: Vec<u8> = file.bytes().collect::<Result<_, _>>().with_location(path)?;

    let file_text = match String::from_utf8(bytes) {
        Ok(f) => f,
        Err(err) => {
            debug!("Utf-8 error: {err:?}");
            println!(
                "Binary format, unable to generate diff, {}",
                to_printable(path)
            );
            return Ok(None);
        }
    };

    Ok(Some(file_text))
}

fn to_printable(path: &Path) -> String {
    let p = path.to_string_lossy();
    if path.is_dir() {
        format!("{p}/")
    } else {
        p.to_string()
    }
}
