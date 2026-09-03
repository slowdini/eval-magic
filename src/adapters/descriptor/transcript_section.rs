use std::io;
use std::path::Path;

use crate::adapters::capabilities::TranscriptParser;
use crate::adapters::extract::ExtractSpec;
use crate::adapters::{PermissionDenial, SessionSurface, TranscriptSummary};
use crate::core::ToolInvocation;
use serde::{Deserialize, Serialize};

use super::{default_true, is_true};

/// A deterministic staged-skill access encoded in a successful native command
/// item rather than a dedicated skill tool event.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillAccessSection {
    pub tool: String,
    pub command_arg: String,
    pub exit_code_arg: String,
    pub read_commands: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptSection {
    pub events_filename: String,
    /// Named primary-summary parser — for streams that need stitching (keyed
    /// joins, content coercion). It cannot coexist with summary extract fields,
    /// but may coexist with an auxiliary session-surface mapping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser: Option<TranscriptParser>,
    /// Optional named parser for permission-denial evidence. When absent, a
    /// primary named parser retains its historical bundled behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_denials_parser: Option<TranscriptParser>,
    /// Declarative extractors by normalized output. Summary fields are the
    /// alternative primary tier; session surface is auxiliary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extract: Option<ExtractSpec>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub surfaces_skill_invocation: bool,
    /// Tool name of a deterministic native skill-invocation event (default
    /// `"Skill"` — Claude Code's Skill tool). Mutually exclusive with
    /// [`Self::skill_access`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_tool: Option<String>,
    /// Argument of the skill-invocation tool that carries the staged slug
    /// (default `"skill"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_arg: Option<String>,
    /// Successful shell-command signature whose literal argument must equal
    /// the task's exact staged `SKILL.md` path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_access: Option<SkillAccessSection>,
}

impl TranscriptSection {
    /// Parse the events file into ordered tool invocations, through whichever
    /// primary-summary tier the descriptor declares.
    pub(crate) fn parse(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        match (&self.parser, &self.extract) {
            (Some(parser), _) => parser.parse(path),
            (None, Some(extract)) => crate::adapters::extract::parse(extract, path),
            // Validation guarantees one tier is declared; fail like a
            // parser-less harness if a section slips through unchecked.
            (None, None) => Err(unwired_error()),
        }
    }

    /// Parse the events file into a full [`TranscriptSummary`].
    pub(crate) fn parse_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        match (&self.parser, &self.extract) {
            (Some(parser), _) => parser.parse_full(path),
            (None, Some(extract)) => crate::adapters::extract::parse_full(extract, path),
            (None, None) => Err(unwired_error()),
        }
    }

    /// Whether the section can identify refused tool calls. Only named readers
    /// can: declarative summary/surface extraction does not describe the
    /// harness's permission model.
    pub(crate) fn surfaces_permission_denials(&self) -> bool {
        self.permission_denials_parser.or(self.parser).is_some_and(
            crate::adapters::capabilities::TranscriptParser::surfaces_permission_denials,
        )
    }

    /// Parse refused tool calls associated with the events path. Unlike
    /// [`parse`](Self::parse), a section without the capability returns an empty
    /// vec rather than an error — no detection is a supported fallback.
    pub(crate) fn parse_permission_denials(
        &self,
        path: &Path,
    ) -> io::Result<Vec<PermissionDenial>> {
        match self.permission_denials_parser.or(self.parser) {
            Some(parser) => parser.parse_permission_denials(path),
            None => Ok(Vec::new()),
        }
    }

    /// Whether this section can report the session's skill/plugin surface,
    /// either declaratively or through a legacy named-parser capability.
    pub(crate) fn surfaces_session_surface(&self) -> bool {
        self.extract
            .as_ref()
            .and_then(|extract| extract.session_surface.as_ref())
            .is_some()
            || self.parser.is_some_and(
                crate::adapters::capabilities::TranscriptParser::surfaces_session_surface,
            )
    }

    /// Parse the session's skill/plugin surface. An explicit declarative
    /// mapping is authoritative; the named-parser arm remains as compatibility
    /// for descriptors authored before that mapping existed.
    pub(crate) fn parse_session_surface(&self, path: &Path) -> io::Result<Option<SessionSurface>> {
        if let Some(surface) = self
            .extract
            .as_ref()
            .and_then(|extract| extract.session_surface.as_ref())
        {
            return crate::adapters::extract::parse_session_surface(surface, path);
        }
        match self.parser {
            Some(parser) => parser.parse_session_surface(path),
            None => Ok(None),
        }
    }
}

fn unwired_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "[transcript] declares neither a parser nor an extract block",
    )
}
