//! Vended plugins (`strands-ts/src/vended-plugins`): optional, batteries-included agent
//! extensions built on the public [`Plugin`](crate::plugins::Plugin) surface.

pub mod context_injector;
pub mod goal;
pub mod skills;

pub use context_injector::{ContextInjector, InjectionContext, InjectionTrigger};
pub use goal::{
    GoalAttempt, GoalLoop, GoalResult, GoalStopReason, JudgeConfig, JudgeOutcome, ValidationContext,
    ValidationOutcome,
};
pub use skills::{AgentSkills, Skill, SkillSource};
