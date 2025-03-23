use std::{
    collections::{HashMap, HashSet},
    fs::{self, read_dir, DirEntry},
    io::{self, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use similar::TextDiff;
use tempdir::TempDir;

use crate::{
    builder::build_tree,
    error::{ErrorLocation, Errors},
    Config,
};

pub async fn calculate_diff(cfg: &Config, line_changes: bool) -> Result<(), Errors> {
    let temp_dir = TempDir::new("dofiles_diff_dir").with_location(&Path::new("/tmp"))?;

    let new_cfg = cfg.with_build_path(temp_dir.path());

    // Build the repository in a temporary directory such that templates are rendered properly etc.
    info!("Building tree in {temp_dir:?}");
    build_tree(&new_cfg).await?;

    let diffs = diff_dir(temp_dir.path(), &cfg.link_dir, &PathBuf::new())?;

    println!("Exists only locally:");
    for add in diffs.only_in_local.iter() {
        println!(" + {}", to_printable(&cfg.link_dir.join(add)));
    }

    println!("Exists only in repo:");
    for miss in diffs.only_in_repo.iter() {
        println!(" - {}", to_printable(&temp_dir.path().join(miss)));
    }

    println!("File exists in both but have been modified:");
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
            println!(
                " file differs {} != {}",
                to_printable(&local),
                to_printable(&repo)
            );
            if line_changes {
                println!("differences: ");
                print_file_diffs(&local, &repo)?;
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

impl Diffs {
    fn join(&mut self, other: Self) {
        other.only_in_repo.into_iter().for_each(|p| {
            self.only_in_repo.insert(p);
        });

        other.only_in_local.into_iter().for_each(|p| {
            self.only_in_local.insert(p);
        });

        other.in_both.into_iter().for_each(|p| {
            self.in_both.insert(p);
        });
    }
}

fn diff_dir(repo_base: &Path, local_base: &Path, relative: &Path) -> Result<Diffs, Errors> {
    let repo = repo_base.join(relative);
    let local = local_base.join(relative);

    debug!("diffing {repo:?} and {local:?}");

    let repo_dirs = get_dirs_map(repo_base, &repo)?;
    let local_dirs = get_dirs_map(local_base, &local)?;

    let mut diffs = Diffs {
        only_in_repo: HashSet::new(),
        only_in_local: HashSet::new(),
        in_both: HashSet::new(),
    };

    for (rel_path, repo_entry) in repo_dirs.into_iter() {
        let local_entry = match local_dirs.get(&rel_path) {
            Some(e) => e,
            None => {
                debug!("Adding to only_in_repo as it was not found in local: {rel_path:?}");
                diffs.only_in_repo.insert(rel_path.clone());
                continue;
            }
        };

        if local_entry.path().is_dir() != repo_entry.path().is_dir() {
            // They have the same name but one is a dir and the other is not.
            debug!("Adding to local and repo {rel_path:?} as their type differs");
            diffs.only_in_local.insert(rel_path.clone());
            diffs.only_in_repo.insert(rel_path);
            continue;
        }

        if local_entry.path().is_dir() {
            // Both local and repo are dirs, recurse.
            let inner = diff_dir(repo_base, local_base, &rel_path)?;
            diffs.join(inner);
        }

        debug!("Adding to in_both as it existed in both: {rel_path:?}");
        diffs.in_both.insert(rel_path);
    }

    debug!("In both is done {:?}", diffs.in_both);

    // Only in repo and in_both should now be populated, however, only_in_local may not be.
    for path in local_dirs.into_keys() {
        if !diffs.only_in_repo.contains(&path) && !diffs.in_both.contains(&path) {
            debug!("Adding to only_in_local as it was not found in either only_in_repo or in_both {path:?}");
            diffs.only_in_local.insert(path);
        }
    }

    Ok(diffs)
}

fn get_dirs_map(base: &Path, full: &Path) -> Result<HashMap<PathBuf, DirEntry>, Errors> {
    let mut walker = read_dir(full).with_location(full)?;

    let mut map = HashMap::new();

    while let Some(entry) = walker.next() {
        let entry = entry.with_location(full)?;

        let full_path = entry.path();
        let relative_path = full_path.strip_prefix(base).with_location(&entry.path())?;
        map.insert(relative_path.to_path_buf(), entry);
    }

    Ok(map)
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
                    println!(" - {}: {line}", start + i);
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
                    println!(" + {}: {line}", start + i);
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
