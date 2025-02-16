use crate::error::{Error, ErrorLocation, Errors, InnerError};
use crate::Config;
use async_recursion::async_recursion;
use blueprint::{parse_template, Env, Value};
use futures::future::join_all;
use futures::TryFutureExt;
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::str::from_utf8;
use tokio::fs::{copy, create_dir, read_dir, read_to_string, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::join;
use tokio::process::Command;

pub const TEMPLATE_EXTENSION: &str = "tpl";

pub async fn build_tree(cfg: &Config) -> Result<(), Errors> {
    let mut env = Env::new();
    env.insert("hostname".into(), Value::Str(get_hostname().await));
    env.insert("username".into(), Value::Str(get_username()));
    env.insert("os".into(), Value::Str(get_operating_system().await));

    debug!("trying to read {:?}", cfg.variables_path);
    if let Ok(s) = read_to_string(&cfg.variables_path).await {
        debug!("parsing {:?}", cfg.variables_path);
        let variables: HashMap<String, toml::Value> =
            toml::de::from_str(&s).with_location(&cfg.variables_path)?;

        for (key, toml_value) in variables {
            let value = match toml_value {
                toml::Value::String(s) => Value::Str(s),
                toml::Value::Boolean(b) => Value::Bool(b),
                _ => return Err(InnerError::Type.with_location(&cfg.variables_path).into()),
            };

            env.insert(key, value);
        }
    } else {
        debug!("failed to read {:?}", cfg.variables_path);
    }

    for flag in &cfg.flags {
        env.insert(flag.to_string(), Value::Bool(true));
    }

    info!("env:");
    for (k, v) in &env {
        info!("  {}: {:?}", k, v);
    }

    dir(cfg, &env, PathBuf::new()).await
}

#[async_recursion]
async fn dir(cfg: &Config, env: &Env, relative: PathBuf) -> Result<(), Errors> {
    let template_path = cfg.template_dir.join(&relative);
    let build_path = cfg.build_dir.join(&relative);

    info!("traversing {:?}", template_path);

    match create_dir(&build_path).await {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.with_location(&build_path).into()),
    }

    let mut walker = read_dir(&template_path)
        .await
        .with_location(&template_path)?;

    let mut dir_tasks = vec![];
    let mut file_tasks = vec![];

    while let Some(entry) = walker.next_entry().await.with_location(&template_path)? {
        let meta = entry.metadata().await.with_location(&entry.path())?;
        let new_relative = relative.join(entry.file_name());

        if meta.is_dir() {
            dir_tasks.push(dir(cfg, env, new_relative));
        } else if meta.is_file() {
            file_tasks.push(file(cfg, env, new_relative));
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

async fn file(cfg: &Config, env: &Env, relative: PathBuf) -> Result<(), Error> {
    let template_path = cfg.template_dir.join(&relative);
    let mut new_path = cfg.build_dir.join(&relative);

    debug!("rendering {:?}", template_path);

    if template_path.extension() == Some(OsStr::new(TEMPLATE_EXTENSION)) {
        // perform templating
        let mut template_file = File::open(&template_path)
            .await
            .with_location(&template_path)?;

        let mut file_str = String::new();
        template_file
            .read_to_string(&mut file_str)
            .await
            .with_location(&template_path)?;

        let permissions = template_file
            .metadata()
            .await
            .with_location(&template_path)?
            .permissions();

        let mut rendered = Vec::<u8>::new();
        parse_template(&file_str)
            .with_location(&template_path)?
            .write(env, &mut rendered)
            .with_location(&template_path)?;
        let rendered = std::str::from_utf8(&rendered).unwrap();

        // remove template file extension
        new_path.set_extension("");

        let mut rendered_file = File::create(&new_path).await.with_location(&new_path)?;

        // write the rendered file
        rendered_file
            .write_all(rendered.as_bytes())
            .await
            .with_location(&new_path)?;

        // make sure the permissions match the original
        rendered_file
            .set_permissions(permissions)
            .await
            .with_location(&new_path)?;
    } else {
        // else just copy the file
        debug!("copying {template_path:?} -> {new_path:?}");
        copy(&template_path, &new_path)
            .await
            .with_location(&template_path)?;
    }

    Ok(())
}

fn get_username() -> String {
    env::var("USER")
        .ok()
        .or_else(|| env::var("USERNAME").ok())
        .unwrap_or_default()
}

async fn get_hostname() -> String {
    async fn read_hostname_file() -> eyre::Result<String> {
        Ok(read_to_string("/etc/hostname").await?)
    }

    async fn run_hostname_cmd() -> eyre::Result<String> {
        Command::new("hostname")
            .output()
            .await
            .map_err(Into::into)
            .and_then(|out| Ok(from_utf8(&out.stdout).map(str::to_string)?))
    }

    read_hostname_file()
        .or_else(|_| run_hostname_cmd())
        .await
        .unwrap_or(String::new())
        .trim()
        .to_string()
}

async fn get_operating_system() -> String {
    Command::new("uname")
        .output()
        .await
        .ok()
        .as_ref()
        .and_then(|out| from_utf8(&out.stdout).ok())
        .unwrap_or("unknown")
        .trim()
        .to_lowercase()
}
