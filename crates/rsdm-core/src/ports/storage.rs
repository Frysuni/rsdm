use thiserror::Error;

pub trait UserStore {
    fn load_remembered_username(&self) -> Result<Option<String>, StorageError>;
    fn store_remembered_username(&self, username: &str) -> Result<(), StorageError>;
    fn load_remembered_session(&self) -> Result<Option<String>, StorageError>;
    fn store_remembered_session(&self, session_id: &str) -> Result<(), StorageError>;
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O error: {0}")]
    Io(String),
    #[error("storage data is invalid: {0}")]
    Invalid(String),
}
