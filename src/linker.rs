use crate::error::{Error, ErrorLocation, Errors};
use crate::Config;
use async_recursion::async_recursion;
use futures::future::join_all;
use std::io::ErrorKind;
use std::path::PathBuf;
use tokio::fs::{create_dir, read_dir, remove_file, symlink};
use tokio::join;

pub async fn link_tree(cfg: &Config) -> Result<(), Errors> {
    dir(cfg, PathBuf::new()).await
}

#[async_recursion]
async fn dir(cfg: &Config, relative: PathBuf) -> Result<(), Errors> {
    let build_path = cfg.build_dir.join(&relative);
    let link_path = cfg.link_dir.join(&relative);

    info!("traversing {:?} ({link_path:?})", build_path);

    match create_dir(&link_path).await {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.with_location(&link_path).into()),
    }

    let mut walker = read_dir(&build_path).await.with_location(&build_path)?;

    let mut dir_tasks = vec![];
    let mut file_tasks = vec![];

    while let Some(entry) = walker.next_entry().await.with_location(&build_path)? {
        let meta = entry.metadata().await.with_location(&entry.path())?;
        let new_relative = relative.join(entry.file_name());

        if meta.is_dir() {
            dir_tasks.push(dir(cfg, new_relative));
        } else if meta.is_file() {
            file_tasks.push(file(cfg, new_relative));
        }
    }

    let dirs = async { join_all(dir_tasks).await.into_iter().collect::<Vec<_>>() };
    let files = async { join_all(file_tasks).await.into_iter().collect::<Vec<_>>() };
    let (dirs, files) = join!(dirs, files);

    let mut errors: Errors = files
        .into_iter()
        .filter_map(|r| r.err())
        .collect::<Vec<_>>()
        .into();

    for error in dirs.into_iter().filter_map(|r| r.err()) {
        errors.join(error);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

async fn file(cfg: &Config, relative: PathBuf) -> Result<(), Error> {
    let build_path = cfg.build_dir.join(&relative);
    let link_path = cfg.link_dir.join(&relative);

    match remove_file(&link_path).await {
        Ok(_) => {
            debug!("removed existing file {:?}", link_path);
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.with_location(&link_path)),
    };

    debug!("linking {:?} to {:?}", link_path, build_path);
    let symlink_content = if build_path.is_absolute() {
        build_path
    } else {
        // TODO: this probably doesn't work for paths containing ".."
        // TODO: this doesn't work if link path is absolute
        let mut relative_symlink = PathBuf::new();
        for _ in link_path.iter().skip(1).filter(|&c| c == ".") {
            relative_symlink.push("..");
        }
        relative_symlink.push(&build_path);

        relative_symlink
    };

    symlink(symlink_content, &link_path)
        .await
        .with_location(&link_path)?;

    Ok(())
}
