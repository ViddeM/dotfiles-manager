use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Default)]
pub struct Errors {
    errors: Vec<Error>,
}

pub struct Error {
    location: PathBuf,
    inner: InnerError,
}

#[derive(Debug, Error)]
pub enum InnerError {
    #[error("IO Error: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to parse template file")]
    Template(#[from] blueprint::Error),

    #[error("Failed to parse toml file")]
    Toml(#[from] toml::de::Error),

    #[error("Unsupported variable type")]
    Type,
}

impl From<Vec<Error>> for Errors {
    fn from(errors: Vec<Error>) -> Self {
        Errors { errors }
    }
}

impl<E> From<E> for Errors
where
    E: Into<Error>,
{
    fn from(error: E) -> Self {
        Errors {
            errors: vec![error.into()],
        }
    }
}

impl Errors {
    pub fn join(&mut self, mut other: Errors) {
        self.errors.append(&mut other.errors);
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn log(self) {
        if self.errors.is_empty() {
            return;
        }

        error!("{} errors occured:", self.errors.len());
        for (i, error) in self.errors.iter().enumerate() {
            error!("  err {:02} at {:?}:", i, error.location);
            error!("      {}", error.inner);
        }
    }
}

pub trait ErrorLocation {
    type Err;
    fn with_location(self, path: &Path) -> Self::Err;
}

impl<T> ErrorLocation for T
where
    T: Into<InnerError>,
{
    type Err = Error;

    fn with_location(self, path: &Path) -> Error {
        Error {
            location: path.to_owned(),
            inner: self.into(),
        }
    }
}

impl<T, E> ErrorLocation for Result<T, E>
where
    E: Into<InnerError>,
{
    type Err = Result<T, Error>;

    fn with_location(self, path: &Path) -> Result<T, Error> {
        self.map_err(|e| Error {
            location: path.to_owned(),
            inner: e.into(),
        })
    }
}
