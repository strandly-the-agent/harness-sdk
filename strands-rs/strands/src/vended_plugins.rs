//! Plugins the SDK ships (TS `vended-plugins/`).

pub mod context_injector;
pub mod skills;

pub use context_injector::{ContextInjector, ContextInjectorConfig, InjectionContext, InjectionTrigger};
pub use skills::{AgentSkills, AgentSkillsConfig, Skill, SkillConfig, SkillSource};
