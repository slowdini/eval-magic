//! Core domain types.
//!
//! The serde-modeled artifacts every pipeline stage reads and writes. Struct
//! field order is the serialized key order, so changing it changes every
//! artifact on disk; keep it stable so artifacts diff cleanly across runs.
//! Types are honest and strict about what each artifact contains, but tolerate
//! unknown fields (no `deny_unknown_fields`) so older artifacts stay readable.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::context::Harness;

/// Meta-assertion id reserved for the skill-invocation check.
pub const SKILL_INVOKED_META_ID: &str = "__skill_invoked";

/// A single assertion attached to an eval, tagged on `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Assertion {
    TranscriptCheck(AssertionTranscriptCheck),
    LlmJudge(AssertionLlmJudge),
    CommandCheck(AssertionCommandCheck),
    DiffScope(AssertionDiffScope),
}

/// A check evaluated against the run transcript (substring/pattern match).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssertionTranscriptCheck {
    pub id: String,
    pub check: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_precede: Option<MustPrecede>,
}

/// An assertion graded by an LLM judge against a rubric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssertionLlmJudge {
    pub id: String,
    pub rubric: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A runner-owned command assertion evaluated against the final task environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssertionCommandCheck {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_files: Option<Vec<String>>,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matrix: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub expect_exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect_stdout: Option<String>,
}

/// A deterministic threshold over the final task environment's diff scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssertionDiffScope {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_files_touched: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_lines_changed: Option<u64>,
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

/// Ordering constraint for a transcript check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MustPrecede {
    CompletionClaim,
    FirstWrite,
    Any,
}

/// One eval case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Eval {
    pub id: String,
    pub prompt: String,
    pub expected_output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    /// Optional source base under `<skill>/evals/`. Fixture destinations remain
    /// the task-relative paths declared in [`Self::files`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertions: Option<Vec<Assertion>>,
    /// Whether the skill-under-test is expected to fire on this eval. Defaults to
    /// true; set false for negative evals where not invoking the skill is correct.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_should_trigger: Option<bool>,
    /// Runs per condition for this eval; overrides the `--runs` flag. Defaults
    /// to the flag's value (1 unless raised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<u32>,
    /// Legacy isolation hint retained for config compatibility. Canonical runs
    /// already give every dispatch a private environment for diff-scope capture,
    /// so `shared` and `isolated` currently have the same effective isolation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,
    /// Ordered scripted user follow-ups. Absence preserves one-shot dispatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<ScriptedTurn>>,
    /// Codebase this eval's task environment is built from, overriding the
    /// config-level default. Appended last so an eval that declares none
    /// serializes exactly as it did before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<CodebaseSource>,
    /// Derives each follow-up from what the agent just said, instead of
    /// scripting them. Mutually exclusive with [`Self::turns`]; absence of both
    /// preserves one-shot dispatch. Appended last so an eval that declares none
    /// serializes exactly as it did before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder: Option<ResponderPolicy>,
    /// Shell-command policy for this eval. When present it replaces the
    /// config-level policy rather than extending it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardPolicyConfig>,
}

/// Authored shell-command allowances for a guarded eval run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardPolicyConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_commands: Vec<String>,
}

/// One scripted user follow-up delivered after an assistant response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptedTurn {
    pub prompt: String,
    pub deliver_when: DeliverWhen,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_response_matches: Option<String>,
}

/// Gate controlling whether a scripted user follow-up is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverWhen {
    Always,
    AgentAsks,
}

/// How the runner answers the agent when an eval has no scripted script to
/// follow: the alternative to [`ScriptedTurn`], not a layer on top of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponderPolicy {
    #[serde(rename = "type")]
    pub kind: ResponderKind,
    /// Maximum follow-up turns the responder may synthesize. The opening prompt
    /// is not one of them, so this counts exactly what `delivered_followups`
    /// counts. `None` takes [`DEFAULT_RESPONDER_MAX_TURNS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

/// The bound a responder eval gets when it declares none: high enough that a
/// real clarifying exchange is not cut short, low enough that an agent stuck in
/// a question loop cannot burn a campaign.
pub const DEFAULT_RESPONDER_MAX_TURNS: u32 = 8;

impl ResponderPolicy {
    /// The bound this policy actually runs under.
    pub fn max_turns(&self) -> u32 {
        self.max_turns.unwrap_or(DEFAULT_RESPONDER_MAX_TURNS)
    }
}

/// Legacy per-eval isolation hint. Every new run is task-scoped regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Shared,
    Isolated,
}

/// Where a task environment's contents come from: a Git repository at an
/// explicit ref, or a directory on this host.
///
/// Untagged because the config spells the two apart by their keys (`url`+`ref`
/// versus `path`) rather than by a discriminator. `evals.schema.json` rejects
/// the ambiguous shapes before serde ever sees them, so the poor error messages
/// untagged enums produce on their own never reach a user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodebaseSource {
    Git {
        url: String,
        /// Required: the runner records the *resolved* SHA, so an eval that
        /// tracked a moving branch could not be re-run against what it measured.
        #[serde(rename = "ref")]
        reference: String,
        #[serde(default)]
        exclude_skill_sources: bool,
    },
    Path {
        path: String,
        #[serde(default)]
        exclude_skill_sources: bool,
    },
}

impl CodebaseSource {
    pub fn exclude_skill_sources(&self) -> bool {
        match self {
            Self::Git {
                exclude_skill_sources,
                ..
            }
            | Self::Path {
                exclude_skill_sources,
                ..
            } => *exclude_skill_sources,
        }
    }
}

/// Whether a source came from a repository URL or a directory on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Git,
    Path,
}

/// A resolved source, as every provenance artifact records it. The codebase a
/// task environment is built from and the skill under test are both recorded
/// through this one shape, so a reader learns them the same way.
///
/// The declared ref is not enough to identify what a run measured — a branch
/// moves — so [`Self::revision`] is the field a report is read against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub kind: SourceKind,
    /// The url or path exactly as declared, so a reader can find it in the config.
    pub source: String,
    /// Where a path source resolved to on the host that ran it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The commit the run actually ran against. Absent only for a directory
    /// that carried no history to name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// The source repository's `origin`. For a host-local path this is the only
    /// handle another reader can resolve: `origin_url` + `revision` names the
    /// same tree anywhere, where `source` names it only here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
    pub branch: String,
    /// Set when the source cannot be resolved off the host that ran it, so a
    /// published claim citing it is not reproducible from the config alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub host_local: bool,
    /// Set when the copy this record describes carries uncommitted work from its
    /// source, so [`Self::revision`] alone does not name what ran. A codebase is
    /// checked out at a commit and is never dirty; a skill is copied as it sits
    /// on disk, which is the point of it, and can be.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
}

/// The resolved skill under test, plus the sibling skills staged alongside it.
///
/// The roster is recorded here rather than rescanned per environment: it is a
/// property of the resolution, and a later scan of the live tree could disagree
/// with what the run actually staged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSource {
    #[serde(flatten)]
    pub source: SourceRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<String>,
}

/// One resolved codebase plus the evals built from it. `conditions.json` and
/// `benchmark.json` carry a list of these; a `run.json` carries the bare
/// [`CodebaseRecord`], having exactly one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodebaseRecord {
    #[serde(flatten)]
    pub source: SourceRecord,
    #[serde(default)]
    pub exclude_skill_sources: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodebaseUse {
    #[serde(flatten)]
    pub codebase: CodebaseRecord,
    pub evals: Vec<String>,
}

/// The parsed `evals.json` for one skill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalsConfig {
    pub skill_name: String,
    /// Default codebase for every eval in this config; a per-eval `codebase`
    /// overrides it. Mirrors how `runs` defaults and is overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<CodebaseSource>,
    pub evals: Vec<Eval>,
    /// Default shell-command policy for evals that do not replace it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardPolicyConfig>,
}

impl EvalsConfig {
    /// Return the authored policy effective for `eval`. A per-eval block is a
    /// complete replacement, including when it is empty.
    pub fn guard_for<'a>(&'a self, eval: &'a Eval) -> Option<&'a GuardPolicyConfig> {
        eval.guard.as_ref().or(self.guard.as_ref())
    }
}

/// A skill staged and discoverable for an eval — its natural name, on-disk
/// `SKILL.md` path, and frontmatter description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableSkill {
    pub name: String,
    pub path: String,
    pub description: String,
}

/// One condition in a comparison run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionEntry {
    pub name: String,
    pub skill_path: Option<String>,
    /// Optional and nullable: absent (omitted), explicit `null`, or a slug.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_key"
    )]
    pub staged_skill_slug: Option<Option<String>>,
}

/// Tri-state field deserializer: a present key — even an explicit `null` —
/// becomes `Some(inner)`, while a missing key falls back to `None` via
/// `default`. The stock `Option<Option<T>>` impl collapses `null` to the outer
/// `None`, which would drop the key on re-serialization and break artifact
/// round-trips.
fn deserialize_present_key<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// The conditions manifest written for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionsRecord {
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    pub conditions: Vec<ConditionEntry>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<Harness>,
    /// Per-run nonce; namespaces dispatch descriptions so they stay unique across
    /// iterations of the same skill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_nonce: Option<String>,
    /// The `--runs` value the iteration was built with (provenance; per-eval
    /// `runs` overrides may raise or lower individual cells).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<u32>,
    /// Operator-declared agent model (provenance; the runner never dispatches
    /// the agent itself, so it cannot observe this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_model: Option<String>,
    /// Resolved descriptor defaults plus run-level overrides applied only to
    /// eval-agent dispatches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_env: BTreeMap<String, String>,
    /// Operator-declared judge model (provenance, like `agent_model`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    /// Operator-declared responder model (provenance, like `agent_model`). A
    /// responder eval puts a third model in the attribution picture, so a
    /// report that names the agent and the judge has to name this one too.
    /// Appended last so a record written before it existed still round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder_model: Option<String>,
    /// Operator-declared provenance label, surfaced in `BASELINE.md` on promote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Codebases the iteration's environments were built from. Empty for a
    /// fixture-only iteration, which keeps its `conditions.json` unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codebases: Vec<CodebaseUse>,
    /// The skill under test, as the run resolved and copied it. Appended last,
    /// and omitted when absent, so a record written before skills were sourced
    /// still round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_source: Option<SkillSource>,
}

/// Comparison mode for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    NewSkill,
    Revision,
}

/// One tool call captured from a run transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    // `ordinal` is serialized before `result`: the adapters construct each
    // invocation without a result and attach it when the matching tool_result
    // arrives, so artifacts list the call before its outcome.
    pub ordinal: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

/// A single subagent run — the artifact bridging dispatch to grading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub eval_id: String,
    pub condition: String,
    pub skill_path: Option<String>,
    pub prompt: String,
    pub files: Vec<String>,
    pub final_message: String,
    pub tool_invocations: Vec<ToolInvocation>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    /// 1-based run index within a multi-run cell; absent for single-run cells.
    /// Appended last so legacy single-run records serialize byte-identically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_index: Option<u32>,
    /// Ordered multi-turn evidence and scripted-delivery outcome. Absent for
    /// legacy one-shot runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationRecord>,
    /// The codebase this run's environment was built from. Grading reads
    /// `run.json` and nothing else, so a result can only be tied to a tree if
    /// the record names one. Appended last, and omitted when absent, so a
    /// fixture-only record serializes as it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<CodebaseRecord>,
    /// The skill under test this run staged. Grading reads `run.json` and nothing
    /// else, so a result can only be tied to a skill revision if the record names
    /// one. Appended last, and omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_source: Option<SkillSource>,
}

/// The completed outcome of one dispatched task's conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub status: ConversationStatus,
    pub delivered_followups: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<ConversationStopReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_before_followup: Option<u32>,
    /// The round the dispatch was killed in, when it outran its deadline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timed_out_in_round: Option<u32>,
    pub events: Vec<ConversationEvent>,
    /// How the responder ended the conversation, when it was the responder that
    /// ended it. Absent for a scripted or one-shot task, for a timeout, and for
    /// `max_turns_reached` — the bound is the runner's decision, not a verdict.
    /// Appended last so a record written before it existed still round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder_outcome: Option<ResponderOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStatus {
    Completed,
    /// Halted at a scripted gate — a normal, recorded result.
    Stopped,
    /// Killed at its deadline. Recorded rather than lost, so the campaign shows
    /// what hung instead of silently missing a cell.
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStopReason {
    AgentDidNotAsk,
    AgentResponseMismatch,
    /// The responder did not produce a usable reply — it declined, its dispatch
    /// failed, or what it wrote failed validation. The specific cause is on the
    /// record's [`ResponderOutcome`]. One reason covers all of them because the
    /// outcome is the same: the run ended mid-task rather than being handed a
    /// reply nobody vouched for.
    ResponderCannotAnswer,
    /// The agent was still asking when the responder's `max_turns` bound was
    /// reached. A bounded conversation, not a failed one.
    MaxTurnsReached,
}

/// One globally ordered event across every delivered conversation round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationEvent {
    UserMessage {
        ordinal: u32,
        round: u32,
        text: String,
        /// How a responder derived this turn. Absent on the seeded eval prompt
        /// and on scripted turns, which are authored rather than derived — the
        /// absence is what tells the two apart. Appended last so a scripted
        /// conversation serializes exactly as it did before the field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<TurnOrigin>,
    },
    AssistantMessage {
        ordinal: u32,
        round: u32,
        text: String,
    },
    ToolInvocation {
        ordinal: u32,
        round: u32,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
}

/// Where a synthesized user turn came from, recorded on the turn itself so a
/// reader can audit whether the responder distorted the run. Absent on the
/// seeded eval prompt and on scripted turns, which are authored, not derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnOrigin {
    pub responder: ResponderKind,
    /// One line from the responder on why it answered this way. Absent when it
    /// offered none; the tag above is what marks the turn derived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// Which responder produced a turn. Named in the record even though there is
/// one of them, because a record outlives the version that wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponderKind {
    Llm,
}

/// How the responder brought a conversation to an end, recorded once on the
/// conversation rather than on a turn — no turn was delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponderOutcome {
    pub ending: ResponderEnding,
    /// Why no usable reply was produced. Absent for [`ResponderEnding::Done`],
    /// where nothing went wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<ResponderStopCause>,
    /// The responder's own one-line account, when it produced one. A dispatch
    /// that never answered has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// The two ways a responder ends a conversation. Deliberately not the parsed
/// verdict, which also carries an answer: an answer is recorded on the turn it
/// became, so it cannot reach here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponderEnding {
    /// The responder judged the agent finished and waiting on nothing.
    Done,
    /// No usable reply, for the reason in [`ResponderOutcome::cause`].
    CannotAnswer,
}

/// Why the responder produced no usable reply. Every variant stops the run with
/// [`ConversationStopReason::ResponderCannotAnswer`]; naming the cause is what
/// lets an operator tell an honest refusal from a broken dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponderStopCause {
    /// The responder said it could not answer without inventing something.
    Declined,
    /// The harness command exited nonzero or could not be spawned.
    DispatchFailed,
    /// The harness command outran the consultation budget.
    DispatchTimedOut,
    /// The dispatch succeeded but wrote no verdict file, or an empty one.
    MissingVerdict,
    /// The verdict file did not parse, or named a verdict that does not exist.
    MalformedVerdict,
    /// An `answer` verdict whose reply was blank.
    EmptyReply,
    /// The reply exceeded the byte cap — a simulated user answers in sentences,
    /// so a long one means the responder started doing the agent's work.
    ReplyTooLong,
    /// The reply carried a fenced code block, for the same reason.
    ReplyContainsCode,
    /// The reply repeated the previous one verbatim: the exchange is circling,
    /// and spending the remaining turns on it would only cost more.
    ReplyRepeated,
}

impl ResponderStopCause {
    /// The cause's serialized name. Warnings print this rather than prose so
    /// what an operator reads is what they would grep the artifacts for.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Declined => "declined",
            Self::DispatchFailed => "dispatch_failed",
            Self::DispatchTimedOut => "dispatch_timed_out",
            Self::MissingVerdict => "missing_verdict",
            Self::MalformedVerdict => "malformed_verdict",
            Self::EmptyReply => "empty_reply",
            Self::ReplyTooLong => "reply_too_long",
            Self::ReplyContainsCode => "reply_contains_code",
            Self::ReplyRepeated => "reply_repeated",
        }
    }
}

/// The result of grading one assertion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssertionResult {
    pub id: String,
    pub passed: bool,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grader: Option<Grader>,
}

/// Which grader produced an assertion result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grader {
    TranscriptCheck,
    LlmJudge,
    CommandCheck,
    DiffScope,
}

/// The full grading output for one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradingResult {
    pub assertion_results: Vec<AssertionResult>,
    // Substantive results + summary first, then the optional meta block —
    // grading.json reads as "the verdict, then the validity check on it".
    pub summary: GradingSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_results: Option<Vec<AssertionResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_summary: Option<MetaSummary>,
}

/// Pass/fail tallies for the main assertions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradingSummary {
    pub passed: u32,
    pub failed: u32,
    pub total: u32,
    pub pass_rate: f64,
}

/// Tallies for the meta-assertions, plus the skill-invocation determination.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetaSummary {
    pub passed: u32,
    pub failed: u32,
    pub total: u32,
    /// `None` (serialized `null`) when invocation could not be determined.
    pub skill_invoked: Option<bool>,
}

/// Token/duration provenance for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Option<i64>>,
    /// Where the numbers came from. `completion-event` = captured live from the
    /// harness's task-completion event; `transcript` = derived from the persisted
    /// transcript using the harness's normalization rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<TimingSource>,
}

/// Provenance of a [`TimingRecord`]'s numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimingSource {
    CompletionEvent,
    Transcript,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context::Harness;
    use serde_json::{Value, json};

    #[test]
    fn assertion_transcript_check_roundtrips_and_tags() {
        let json_in = json!({"id": "a", "type": "transcript_check", "check": "ran tests"});
        let parsed: Assertion = serde_json::from_value(json_in).unwrap();
        match &parsed {
            Assertion::TranscriptCheck(c) => {
                assert_eq!(c.id, "a");
                assert_eq!(c.check, "ran tests");
                assert!(c.pattern.is_none());
                assert!(c.must_precede.is_none());
            }
            _ => panic!("expected transcript_check variant"),
        }
        let out = serde_json::to_value(&parsed).unwrap();
        assert_eq!(
            out.get("type"),
            Some(&Value::String("transcript_check".into()))
        );
        // Absent optionals are omitted, not emitted as null.
        assert!(out.get("pattern").is_none());
        assert!(out.get("must_precede").is_none());
    }

    #[test]
    fn assertion_llm_judge_tag() {
        let parsed: Assertion =
            serde_json::from_value(json!({"id": "j", "type": "llm_judge", "rubric": "is correct"}))
                .unwrap();
        assert!(matches!(parsed, Assertion::LlmJudge(_)));
        let out = serde_json::to_value(&parsed).unwrap();
        assert_eq!(out.get("type"), Some(&Value::String("llm_judge".into())));
    }

    #[test]
    fn eval_omits_absent_optionals() {
        let eval = Eval {
            id: "e1".into(),
            prompt: "p".into(),
            expected_output: "o".into(),
            files: None,
            files_root: None,
            assertions: None,
            skill_should_trigger: None,
            runs: None,
            isolation: None,
            turns: None,
            codebase: None,
            responder: None,
            guard: None,
        };
        let out = serde_json::to_value(&eval).unwrap();
        assert!(out.get("files").is_none());
        assert!(out.get("files_root").is_none());
        assert!(out.get("assertions").is_none());
        assert!(out.get("skill_should_trigger").is_none());
        assert!(out.get("runs").is_none());
        assert!(out.get("isolation").is_none());
    }

    #[test]
    fn isolation_round_trips_snake_case() {
        let eval = Eval {
            id: "e1".into(),
            prompt: "p".into(),
            expected_output: "o".into(),
            files: None,
            files_root: None,
            assertions: None,
            skill_should_trigger: None,
            runs: None,
            isolation: Some(Isolation::Isolated),
            turns: None,
            codebase: None,
            responder: None,
            guard: None,
        };
        let out = serde_json::to_value(&eval).unwrap();
        assert_eq!(
            out.get("isolation"),
            Some(&Value::String("isolated".into()))
        );
        let back: Eval = serde_json::from_value(out).unwrap();
        assert_eq!(back.isolation, Some(Isolation::Isolated));
    }

    #[test]
    fn run_record_skill_path_null_emitted() {
        let rec = RunRecord {
            eval_id: "e".into(),
            condition: "with-skill".into(),
            skill_path: None,
            prompt: "p".into(),
            files: vec![],
            final_message: "done".into(),
            tool_invocations: vec![],
            total_tokens: None,
            duration_ms: None,
            run_index: None,
            conversation: None,
            codebase: None,
            skill_source: None,
        };
        let out = serde_json::to_value(&rec).unwrap();
        // Required-but-nullable keys are present with a null value.
        assert_eq!(out.get("skill_path"), Some(&Value::Null));
        assert_eq!(out.get("total_tokens"), Some(&Value::Null));
        assert_eq!(out.get("duration_ms"), Some(&Value::Null));
        // Absent run_index is omitted, keeping single-run records byte-identical.
        assert!(out.get("run_index").is_none());
        assert!(out.get("conversation").is_none());
    }

    #[test]
    fn meta_summary_skill_invoked_null_emitted() {
        let ms = MetaSummary {
            passed: 0,
            failed: 0,
            total: 0,
            skill_invoked: None,
        };
        let out = serde_json::to_value(ms).unwrap();
        assert_eq!(out.get("skill_invoked"), Some(&Value::Null));
    }

    #[test]
    fn staged_skill_slug_tri_state() {
        let base = |slug| ConditionEntry {
            name: "c".into(),
            skill_path: Some("/p".into()),
            staged_skill_slug: slug,
        };
        // Absent → key omitted.
        let absent = serde_json::to_value(base(None)).unwrap();
        assert!(absent.get("staged_skill_slug").is_none());
        // Explicit null → key present, null.
        let null = serde_json::to_value(base(Some(None))).unwrap();
        assert_eq!(null.get("staged_skill_slug"), Some(&Value::Null));
        // String → key present, string.
        let some = serde_json::to_value(base(Some(Some("slug-1".into())))).unwrap();
        assert_eq!(
            some.get("staged_skill_slug"),
            Some(&Value::String("slug-1".into()))
        );
        // Deserialization preserves all three states (explicit null must stay
        // a present key, not collapse to absent).
        let back = |v| {
            serde_json::from_value::<ConditionEntry>(v)
                .unwrap()
                .staged_skill_slug
        };
        assert_eq!(back(absent), None);
        assert_eq!(back(null), Some(None));
        assert_eq!(back(some), Some(Some("slug-1".into())));
    }

    #[test]
    fn conditions_record_mode_and_harness_render() {
        let rec = ConditionsRecord {
            mode: Mode::NewSkill,
            baseline: None,
            conditions: vec![],
            timestamp: "2026-06-08T00:00:00Z".into(),
            harness: Some(Harness::resolve("claude-code").unwrap()),
            run_nonce: None,
            runs: None,
            agent_model: None,
            agent_env: BTreeMap::new(),
            judge_model: None,
            responder_model: None,
            label: None,
            codebases: Vec::new(),
            skill_source: None,
        };
        let out = serde_json::to_value(&rec).unwrap();
        assert_eq!(out.get("mode"), Some(&Value::String("new-skill".into())));
        assert_eq!(
            out.get("harness"),
            Some(&Value::String("claude-code".into()))
        );
        // Absent optionals omitted.
        assert!(out.get("baseline").is_none());
        assert!(out.get("run_nonce").is_none());
    }

    #[test]
    fn codebase_use_records_effective_skill_source_exclusion() {
        let value = serde_json::json!({
            "kind": "path",
            "source": "../fixture",
            "branch": "work",
            "exclude_skill_sources": true,
            "evals": ["e1"]
        });

        let record: CodebaseUse = serde_json::from_value(value).unwrap();
        let rendered = serde_json::to_value(record).unwrap();

        assert_eq!(rendered["exclude_skill_sources"], true);
    }

    #[test]
    fn timing_source_kebab_roundtrips() {
        let v = serde_json::to_value(TimingSource::CompletionEvent).unwrap();
        assert_eq!(v, Value::String("completion-event".into()));
        let back: TimingSource = serde_json::from_value(v).unwrap();
        assert_eq!(back, TimingSource::CompletionEvent);
    }
}

#[cfg(test)]
#[path = "types/artifact_tests.rs"]
mod artifact_tests;
