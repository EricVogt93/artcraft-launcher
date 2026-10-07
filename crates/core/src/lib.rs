pub mod catalog;
pub mod installer;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod linux_integration;
pub mod manager;
pub mod model;
pub mod persistence;
pub mod platform;
pub mod releases;
pub mod self_update;
pub mod worker;

pub use catalog::{App, CATALOG, app};
pub use manager::Manager;
pub use model::*;
pub use releases::{Github, Remote};
pub use worker::{Command, Event, Worker};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Network(#[from] reqwest::Error),
}
pub fn fail(message: impl Into<String>) -> Error {
    Error::Message(message.into())
}
