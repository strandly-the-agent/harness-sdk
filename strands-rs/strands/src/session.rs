//! Session persistence (TS `session/`): a [`SessionManager`] saves and restores agent snapshots
//! through a [`Storage`](crate::storage::Storage) backend.

mod session_manager;
mod snapshot_storage_adapter;
mod validation;

pub use session_manager::{
    ListSnapshotIdsOptions, ListSnapshotIdsOptionsBuilder, SaveLatestStrategy, SessionManager,
    SessionManagerConfig, SessionManagerConfigBuilder, SnapshotTarget, SnapshotTriggerCallback,
    SnapshotTriggerParams, list_snapshot_ids_options_builder, session_manager_config_builder,
};

#[cfg(test)]
mod conformance_tests;
