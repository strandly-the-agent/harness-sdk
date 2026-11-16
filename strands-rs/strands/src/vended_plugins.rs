//! Plugins the SDK ships (TS `vended-plugins/`).

pub mod context_injector;
pub mod skills;

#[doc(inline)]
pub use context_injector::{ContextInjector, ContextInjectorConfig, InjectionContext, InjectionTrigger};
#[doc(inline)]
pub use skills::{AgentSkills, AgentSkillsConfig, Skill, SkillConfig, SkillSource};
