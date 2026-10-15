//! Session persistence: snapshots of agent state, a pluggable storage trait, and the
//! [`SessionManager`] plugin that restores and saves them around an invocation.

mod agent_snapshot;
mod file_storage;
#[cfg(feature = "s3")]
mod s3_storage;
mod session_manager;
mod storage;
pub(crate) mod types;
mod validation;

pub use file_storage::FileStorage;
#[cfg(feature = "s3")]
pub use s3_storage::{S3SessionStorage, S3SessionStorageBuilder};
pub use session_manager::{
    SaveLatestStrategy, SessionManager, SessionManagerOptions, SnapshotTrigger, DEFAULT_SESSION_ID,
};
pub use storage::{SnapshotLocation, SnapshotStorage};
pub use types::{Scope, Snapshot, SnapshotManifest, SNAPSHOT_SCHEMA_VERSION};
pub use validation::{validate_identifier, validate_scope, validate_uuid_v7};

#[cfg(test)]
pub(crate) mod test_support;
