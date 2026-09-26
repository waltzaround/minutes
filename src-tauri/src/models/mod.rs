//! Model catalog, installation state and downloads.
//!
//! Model weights are never bundled with the installer and never downloaded
//! without an explicit user action.

pub mod catalog;
mod catalog_speech;
pub mod download;
pub mod manager;
