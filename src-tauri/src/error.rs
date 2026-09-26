//! Error type returned across the Tauri boundary.
//!
//! Internal details (SQL errors, subprocess output, stack traces) are logged
//! but never shown to normal users; the frontend receives a stable `code`
//! and a plain-language `message`.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A message that is safe and useful to show the user as-is.
    #[error("{message}")]
    User { code: &'static str, message: String },
    #[error("not found: {0}")]
    NotFound(String),
    /// A feature that is not available in this build/platform yet. Reported
    /// honestly rather than faked.
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn user(code: &'static str, message: impl Into<String>) -> Self {
        AppError::User { code, message: message.into() }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::User { code, .. } => code,
            AppError::NotFound(_) => "not_found",
            AppError::Unavailable(_) => "unavailable",
            AppError::Database(_) => "database",
            AppError::Internal(_) => "internal",
        }
    }

    fn public_message(&self) -> String {
        match self {
            AppError::User { message, .. } => message.clone(),
            AppError::NotFound(what) => format!("{what} could not be found."),
            AppError::Unavailable(msg) => msg.clone(),
            AppError::Database(_) => "Something went wrong while saving your data. Details are in the log.".into(),
            AppError::Internal(_) => "Something went wrong. Details are in the log.".into(),
        }
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl From<crate::storage::secrets::SecretError> for AppError {
    fn from(e: crate::storage::secrets::SecretError) -> Self {
        AppError::user("credential_store", e.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if matches!(self, AppError::Database(_) | AppError::Internal(_)) {
            tracing::error!(error = ?self, "command failed");
        }
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.public_message())?;
        st.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
