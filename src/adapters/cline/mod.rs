//! Cline harness code capabilities.
//!
//! The Cline descriptor (`harnesses/cline.toml`) is data-only except for the
//! named capabilities whose behavior lives here: the `cline-json` transcript
//! parser ([`transcript`]) and the `cline-skills` shadow preflight
//! ([`skill_shadow`]).

pub mod skill_shadow;
pub mod transcript;
