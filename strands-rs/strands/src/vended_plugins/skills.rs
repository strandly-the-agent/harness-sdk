//! Agent Skills (TS `vended-plugins/skills/`): [`Skill`], a parsed `SKILL.md`, and the
//! [`AgentSkills`] plugin, which lists the available skills in the system prompt and loads one
//! on demand through the `skills` tool.

mod agent_skills;
mod skill;

pub use agent_skills::{AgentSkills, AgentSkillsConfig, SkillSource};
pub use skill::{Skill, SkillConfig, SkillOptions};
