//! Tauri commands: the only surface the frontend can use. Each command is
//! narrowly scoped; the UI never touches devices, files, tokens, SQLite or
//! inference runtimes directly.

pub mod analysis;
pub mod app;
pub mod integrations;
pub mod meetings;
pub mod models;
pub mod people;
pub mod system;
