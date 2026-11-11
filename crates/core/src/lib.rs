pub mod catalog;
pub mod diagnostics;
pub mod installer;
pub mod inventory;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod linux_integration;
pub mod manager;
pub mod model;
pub mod persistence;
pub mod platform;
pub mod queue;
pub mod releases;
pub mod self_update;
pub mod storage;
pub mod worker;
pub mod workspace;

pub use catalog::{App, CATALOG, app};
pub use manager::Manager;
pub use model::*;
pub use releases::{Github, Remote};
pub use worker::{Command, Event, Worker};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Multiple application entries found. Select a path relative to the package: {}", .0.join(", "))]
    Entrypoints(Vec<String>),
    #[error("CraftLauncher is already using this library. Close the other instance first.")]
    LibraryLocked,
    #[error("GitHub rate limit reached. Please try again later.")]
    RateLimited,
    #[error("{0}")]
    Cancelled(String),
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Network(#[from] reqwest::Error),
}
impl Error {
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled(_) => true,
            Self::Message(msg) => msg.to_ascii_lowercase().contains("cancelled"),
            _ => false,
        }
    }
}
pub fn fail(message: impl Into<String>) -> Error {
    Error::Message(message.into())
}
pub fn cancelled(message: impl Into<String>) -> Error {
    Error::Cancelled(message.into())
}
