//! Agent Skills (TS `vended-plugins/skills/`): [`Skill`], a parsed `SKILL.md`, and the
//! [`AgentSkills`] plugin, which lists the available skills in the system prompt and loads one
//! on demand through the `skills` tool.

mod skill;

pub use skill::{Skill, SkillConfig, SkillOptions};
