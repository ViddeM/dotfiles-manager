use crate::builder::TEMPLATE_EXTENSION;
use crate::error::{Error, ErrorLocation, Errors};
use crate::Config;
use async_recursion::async_recursion;
use blueprint::parse_template;
use futures::future::join_all;
use std::ffi::OsStr;
use std::path::PathBuf;
use tokio::fs::{read_dir, read_to_string};
use tokio::join;

/// Iterate over the directory tree and print all variables used in all template files.
pub async fn print_variables(cfg: &Config) -> Result<(), Errors> {
    for var in dir(cfg, PathBuf::new()).await? {
        println!("{}", var);
    }

    Ok(())
}

#[async_recursion]
async fn dir(cfg: &Config, relative: PathBuf) -> Result<Vec<String>, Errors> {
    let template_path = cfg.template_dir.join(&relative);

    info!("traversing {:?}", template_path);

    let mut walker = read_dir(&template_path)
        .await
        .with_location(&template_path)?;

    let mut dir_tasks = vec![];
    let mut file_tasks = vec![];

    while let Some(entry) = walker.next_entry().await.with_location(&template_path)? {
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

    let mut vars = vec![];
    let mut errors = vec![];

    for result in files.into_iter() {
        match result {
            Ok(mut more_vars) => vars.append(&mut more_vars),
            Err(error) => errors.push(error),
        }
    }

    let mut errors: Errors = errors.into();

    for result in dirs.into_iter() {
        match result {
            Ok(mut more_vars) => vars.append(&mut more_vars),
            Err(error) => errors.join(error),
        }
    }

    if errors.is_empty() {
        vars.sort_unstable();
        vars.dedup();
        Ok(vars)
    } else {
        Err(errors)
    }
}

async fn file(cfg: &Config, relative: PathBuf) -> Result<Vec<String>, Error> {
    let template_path = cfg.template_dir.join(&relative);

    if template_path.extension() != Some(OsStr::new(TEMPLATE_EXTENSION)) {
        return Ok(vec![]);
    }

    debug!("reading {:?}", template_path);

    // parse template
    let file_str = read_to_string(&template_path)
        .await
        .with_location(&template_path)?;

    let vars = parse_template(&file_str)
        .with_location(&template_path)?
        .list_variables()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    Ok(vars)
}
