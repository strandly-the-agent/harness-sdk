//! Background tool execution (TS `background-tasks/`): tools run as tasks while the agent
//! continues, and their results are delivered as follow-up input.
//!
//! Enable it with the `background_tasks` agent config key; [`BackgroundTasksConfig`] selects
//! which tools may run in the background.

#[expect(
    clippy::module_inception,
    reason = "mirrors TS `background-tasks/background-tasks.ts`"
)]
mod background_tasks;
pub(crate) mod in_process;
mod types;

pub(crate) use background_tasks::{BackgroundTaskScheduler, Route};

pub use types::{BackgroundTasks, BackgroundTasksConfig};
