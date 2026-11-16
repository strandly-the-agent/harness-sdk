//! Session persistence (TS `session/`): a [`SessionManager`] saves and restores agent snapshots
//! through a [`Storage`](crate::storage::Storage) backend.

mod session_manager;
mod snapshot_storage_adapter;
mod validation;

pub use session_manager::{
    ListSnapshotIdsOptions, SaveLatestStrategy, SessionManager, SessionManagerConfig, SnapshotTarget,
    SnapshotTriggerCallback, SnapshotTriggerParams,
};

#[cfg(test)]
mod conformance_tests;
