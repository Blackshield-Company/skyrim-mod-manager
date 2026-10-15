//! Core library for skyrim-mod-manager.
//!
//! * [`detect`]    – find Skyrim SE/AE and its Proton / umu prefix (read-only)
//! * [`instance`]  – managed staging folder: install, enable/disable, priority
//! * [`conflicts`] – which mod wins each overwritten path
//! * [`deploy`]    – dry-run plan, hardlink deploy, manifest-backed undeploy
//! * [`plugins`]   – masters, load-order checks, Plugins.txt

pub mod conflicts;
pub mod deploy;
pub mod detect;
pub mod error;
pub mod instance;
pub mod paths;
pub mod plugins;

pub use error::{Error, Result};
