use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("archive error: {0}")]
    Archive(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin error in {path}: {msg}")]
    Plugin { path: PathBuf, msg: String },
    #[error("mod not found: {0}")]
    ModNotFound(String),
    #[error("mod already exists: {0}")]
    ModExists(String),
    #[error("invalid mod name: {0}")]
    InvalidName(String),
    #[error("already deployed; undeploy first")]
    AlreadyDeployed,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) trait IoCtx<T> {
    fn ctx(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoCtx<T> for std::io::Result<T> {
    fn ctx(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
