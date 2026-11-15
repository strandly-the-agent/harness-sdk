//! Background tool execution (TS `background-tasks/`): tools run as tasks while the agent
//! continues, and their results are delivered as follow-up input.
//!
//! Enable it with the `background_tasks` agent config key; [`BackgroundTasksConfig`] selects
//! which tools may run in the background.

pub(crate) mod in_process;
mod types;

pub(crate) use types::{BackgroundTask, Failure, FailureType, TaskStatus};
pub use types::{BackgroundTasks, BackgroundTasksConfig};
