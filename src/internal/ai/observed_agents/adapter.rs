//! Adapter contracts for external-Agent capture.
//!
//! Designed as **one core trait + several optional capability traits** so a new
//! agent can be wired in with as little as `provider_kind`, `provider_name`,
//! `read_transcript`, and `protected_dirs`. Hooks, transcript truncation, and
//! chunking are all opt-in.
//!
//! See `docs/development/commands/_general.md` (section 5) for the rationale and the v1
//! adapter matrix (Claude Code + Gemini stable; 5 preview stubs).

use std::path::PathBuf;

use anyhow::Result;

use super::capability::{
    DeclaredAgentCaps, HookResponseWriter, ModelExtractor, PromptExtractor, SkillEventExtractor,
    SubagentAwareExtractor, TextGenerator, TokenCalculator, TranscriptAnalyzer,
    TranscriptCompactor, TranscriptPreparer,
};
use crate::internal::ai::hooks::provider::HookProvider;

/// Identity for one of the externally-hosted agents Libra knows how to capture.
///
/// The variant set is closed because every variant maps to a CLI subcommand
/// (`libra agent enable claude-code`, …) and to a column value in
/// `agent_session.agent_kind`. Adding a new agent requires a v2 plan and a
/// migration touching the CHECK constraint on that column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeCode,
    Cursor,
    Codex,
    Gemini,
    OpenCode,
    Copilot,
    FactoryAi,
}

impl AgentKind {
    /// Snake-case identifier used as the `agent_session.agent_kind` value and
    /// in log lines. Stable across releases — downstream tooling joins on it.
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
            Self::FactoryAi => "factory_ai",
        }
    }

    /// Parse the `agent_session.agent_kind` db tag back into an
    /// [`AgentKind`]. Inverse of [`as_db_str`](Self::as_db_str): for
    /// every variant `kind`, `AgentKind::from_db_str(kind.as_db_str())
    /// == Some(kind)`. Returns `None` for tags that don't match a
    /// known variant — callers that read from `agent_session` rows
    /// should treat that as a schema mismatch and fail closed rather
    /// than silently dispatching to the wrong adapter.
    ///
    /// Unlike [`from_cli_slug`](Self::from_cli_slug), this accepts the
    /// snake_case wire form exclusively — db rows have a fixed
    /// canonical shape, while CLI slugs accept aliases. Keep the two
    /// helpers separate so a `agent_session` row that somehow
    /// contains `"claude-code"` (CLI slug shape) fails the lookup
    /// instead of silently round-tripping to `ClaudeCode`.
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "claude_code" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "opencode" => Some(Self::OpenCode),
            "copilot" => Some(Self::Copilot),
            "factory_ai" => Some(Self::FactoryAi),
            _ => None,
        }
    }

    /// Slug used on the CLI (`libra agent enable <slug>`). Hyphenated rather
    /// than snake_case to match the convention of other Libra subcommands.
    pub const fn as_cli_slug(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
            Self::FactoryAi => "factory-ai",
        }
    }

    /// Parse a CLI slug back into a kind. Accepts both hyphen and underscore
    /// forms so users can paste either style. Returns `None` if the input
    /// isn't a recognised agent.
    pub fn from_cli_slug(slug: &str) -> Option<Self> {
        match slug {
            "claude-code" | "claude_code" | "claude" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "copilot" | "github-copilot" => Some(Self::Copilot),
            "factory-ai" | "factory_ai" | "factory" => Some(Self::FactoryAi),
            _ => None,
        }
    }

    /// All variants in registration order. Useful for `libra agent enable`'s
    /// listing path and tests that want to round-trip every kind.
    pub const fn all() -> &'static [Self] {
        &[
            Self::ClaudeCode,
            Self::Cursor,
            Self::Codex,
            Self::Gemini,
            Self::OpenCode,
            Self::Copilot,
            Self::FactoryAi,
        ]
    }
}

/// Stability tier for an [`AgentKind`].
///
/// `Stable` means the v1 adapter implements `read_transcript` and is wired
/// through `libra agent` end-to-end. `Preview` means the agent is reachable
/// from the CLI but its adapter returns `Err(AgentNotYetImplemented)` for the
/// transcript/hook code paths.
///
/// `Serialize` is derived (lowercase via `#[serde(rename_all = "snake_case")]`)
/// because `libra agent doctor --json` emits this enum verbatim. Renaming
/// either variant changes a public CLI contract — bump the JSON schema if
/// you must.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStability {
    Stable,
    Preview,
}

/// Per-call context handed to [`ObservedAgent::read_transcript`] when the
/// runtime asks an adapter for the latest transcript bytes.
///
/// Kept as a small concrete struct (rather than passing the whole
/// `SessionState`) so adapters do not need to depend on the hook runtime's
/// internals.
#[derive(Debug, Clone)]
pub struct AgentSessionCtx {
    /// `agent_session.session_id`.
    pub session_id: String,
    /// `agent_session.provider_session_id` — the agent's own session id, used
    /// by the adapter to locate the transcript file.
    pub provider_session_id: String,
    /// Working directory the session was started in.
    pub working_dir: PathBuf,
    /// Absolute path to the agent's on-disk transcript file (e.g. Claude
    /// Code's session JSONL). Captured from the SessionStart hook
    /// envelope and persisted on the SessionState; the adapter relies on
    /// this to avoid having to reconstruct provider-specific path
    /// conventions (e.g. `~/.claude/projects/<workdir>/<id>.jsonl`).
    /// `None` when no envelope ever provided one.
    pub transcript_path: Option<PathBuf>,
}

/// Reasons an adapter call can fail.
///
/// Adapters return [`anyhow::Error`] from their methods, but the runtime
/// recognises `AgentError::NotYetImplemented` specifically so it can
/// downgrade the failure to a soft warning rather than an error: preview
/// adapters are expected to surface this. Use [`agent_not_yet_implemented`]
/// to construct the canonical instance.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("adapter for '{0}' is preview-only and not yet implemented")]
    NotYetImplemented(&'static str),
}

/// Convenience constructor for the preview-stub `Err` value. Kept as a free
/// function so callers can write `Err(agent_not_yet_implemented(self))?`
/// without importing the variant explicitly.
pub fn agent_not_yet_implemented(agent: &dyn ObservedAgent) -> AgentError {
    AgentError::NotYetImplemented(agent.provider_name())
}

/// Core trait every observed agent implements.
///
/// Boundary condition: [`Self::read_transcript`] returns the agent's *raw*
/// (un-redacted) bytes. The runtime is responsible for piping them through
/// `redaction::Redactor::redact` before any persistence path consumes them.
pub trait ObservedAgent: Send + Sync {
    fn provider_kind(&self) -> AgentKind;
    fn provider_name(&self) -> &'static str;

    /// Stability tier for this adapter. Defaults to [`AgentStability::Stable`]
    /// — preview stubs override.
    fn stability(&self) -> AgentStability {
        AgentStability::Stable
    }

    /// Read the agent's native transcript bytes. `Ok(None)` means "no
    /// transcript is currently available" (e.g. the session has not produced
    /// any output yet); `Err(...)` means the adapter could not access the
    /// transcript.
    ///
    /// The returned bytes are **not yet redacted** — callers must run them
    /// through [`super::redaction::Redactor`] before persistence.
    fn read_transcript(&self, session: &AgentSessionCtx) -> Result<Option<Vec<u8>>>;

    /// Directories owned by the agent that `rewind` and `clean` must leave
    /// alone (`.claude`, `.gemini`, …). Path elements are matched
    /// case-sensitively against the workspace tree walker.
    fn protected_dirs(&self) -> &'static [&'static str];

    // Capability accessors (AG-16). Rust's answer to entire's runtime
    // `As*(agent)` probes: every accessor defaults to `None`, and an adapter
    // that implements a capability overrides the accessor to `Some(self)`.
    // Hook support is expressed through `as_hooks()`; the zero-impl
    // hooks dup trait that used to live here was deleted in favour of the
    // shared `hooks::provider::HookProvider` contract (AG-16).

    /// Hook lifecycle support (E1 `hooks`). Adapters converge in AG-19.
    fn as_hooks(&self) -> Option<&dyn HookProvider> {
        None
    }
    /// E1 `transcript_analyzer`.
    fn as_transcript_analyzer(&self) -> Option<&dyn TranscriptAnalyzer> {
        None
    }
    /// Prompt extraction — no 8-bool key; gated by `transcript_analyzer`.
    fn as_prompt_extractor(&self) -> Option<&dyn PromptExtractor> {
        None
    }
    /// E1 `transcript_preparer`.
    fn as_transcript_preparer(&self) -> Option<&dyn TranscriptPreparer> {
        None
    }
    /// E1 `token_calculator`.
    fn as_token_calculator(&self) -> Option<&dyn TokenCalculator> {
        None
    }
    /// Model extraction — deliberately outside the 8-bool set (E1).
    fn as_model_extractor(&self) -> Option<&dyn ModelExtractor> {
        None
    }
    /// E1 `text_generator`.
    fn as_text_generator(&self) -> Option<&dyn TextGenerator> {
        None
    }
    /// E1 `compact_transcript`.
    fn as_transcript_compactor(&self) -> Option<&dyn TranscriptCompactor> {
        None
    }
    /// E1 `hook_response_writer`.
    fn as_hook_response_writer(&self) -> Option<&dyn HookResponseWriter> {
        None
    }
    /// E1 `subagent_aware_extractor`.
    fn as_subagent_aware_extractor(&self) -> Option<&dyn SubagentAwareExtractor> {
        None
    }
    /// Skill-event projection — deliberately outside the 8-bool set (E1/E7).
    fn as_skill_event_extractor(&self) -> Option<&dyn SkillEventExtractor> {
        None
    }
    /// Transcript truncation (pre-AG-16 capability, kept as-is).
    fn as_transcript_truncator(&self) -> Option<&dyn TranscriptTruncator> {
        None
    }
    /// Transcript chunking (pre-AG-16 capability, kept as-is).
    fn as_transcript_chunker(&self) -> Option<&dyn TranscriptChunker> {
        None
    }

    /// Introspect the E1 8-bool capability declaration from the `as_*`
    /// accessors. Built-in adapters rely on this default; external RPC
    /// shims (AG-18) override it from their negotiated
    /// `CapabilityDeclarer` payload instead.
    fn declared_capabilities(&self) -> DeclaredAgentCaps {
        DeclaredAgentCaps {
            hooks: self.as_hooks().is_some(),
            transcript_analyzer: self.as_transcript_analyzer().is_some(),
            transcript_preparer: self.as_transcript_preparer().is_some(),
            token_calculator: self.as_token_calculator().is_some(),
            compact_transcript: self.as_transcript_compactor().is_some(),
            text_generator: self.as_text_generator().is_some(),
            hook_response_writer: self.as_hook_response_writer().is_some(),
            subagent_aware_extractor: self.as_subagent_aware_extractor().is_some(),
        }
    }
}

/// Optional capability: transcript truncation at a checkpoint boundary.
///
/// Required by `libra agent checkpoint rewind --apply` once Phase 2 lands.
/// V1 adapters do NOT implement this — `rewind --apply` therefore leaves the
/// agent's transcript file untouched and prints a warning, per
/// `docs/development/commands/_general.md` section 7.3.
pub trait TranscriptTruncator: ObservedAgent {
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>>;
}

/// Optional capability: chunking very large transcripts before storage.
///
/// V2 candidate. Listed here so the trait surface is documented; v1 callers
/// don't reach for it because Git packfile delta compression already does the
/// job for the foreseeable size envelope.
pub trait TranscriptChunker: ObservedAgent {
    fn chunk_transcript(&self, content: &[u8], max_size: usize) -> Result<Vec<Vec<u8>>>;
    fn reassemble_transcript(&self, chunks: &[Vec<u8>]) -> Result<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_round_trip() {
        for kind in AgentKind::all() {
            let slug = kind.as_cli_slug();
            assert_eq!(AgentKind::from_cli_slug(slug), Some(*kind));
        }
    }

    #[test]
    fn agent_kind_accepts_underscore_aliases() {
        assert_eq!(
            AgentKind::from_cli_slug("claude_code"),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            AgentKind::from_cli_slug("factory_ai"),
            Some(AgentKind::FactoryAi)
        );
        assert_eq!(
            AgentKind::from_cli_slug("github-copilot"),
            Some(AgentKind::Copilot)
        );
    }

    #[test]
    fn agent_kind_rejects_unknown() {
        assert_eq!(AgentKind::from_cli_slug("not-an-agent"), None);
        assert_eq!(AgentKind::from_cli_slug(""), None);
    }

    /// `from_db_str` is the inverse of `as_db_str` — for every variant,
    /// the round-trip `from_db_str(kind.as_db_str()) == Some(kind)`.
    /// Pin both directions across every variant so a future rename
    /// that touches only one side of the pair fails to compile through
    /// the exhaustive match arms, and silently shipping a desync
    /// becomes impossible.
    #[test]
    fn agent_kind_from_db_str_round_trips_every_variant() {
        for kind in AgentKind::all() {
            assert_eq!(
                AgentKind::from_db_str(kind.as_db_str()),
                Some(*kind),
                "round-trip mismatch for {kind:?}",
            );
        }
    }

    /// `from_db_str` accepts ONLY the snake_case wire form — it does
    /// not fall through to CLI-slug aliases. A `agent_session.agent_kind`
    /// row that somehow contains `"claude-code"` (the CLI slug shape)
    /// should be rejected as a schema mismatch, not silently mapped to
    /// `ClaudeCode`. Pinning the rejection here protects the
    /// dispatch-on-db-tag pattern from accidentally lifting CLI-side
    /// alias permissiveness into the storage layer.
    #[test]
    fn agent_kind_from_db_str_rejects_cli_slug_aliases_and_unknowns() {
        // Hyphenated CLI slugs are rejected even when the underlying
        // kind is real — the wire form must use the snake_case db tag.
        assert_eq!(AgentKind::from_db_str("claude-code"), None);
        assert_eq!(AgentKind::from_db_str("factory-ai"), None);
        // Unknown values return None instead of panicking.
        assert_eq!(AgentKind::from_db_str("not-an-agent"), None);
        assert_eq!(AgentKind::from_db_str(""), None);
    }

    #[test]
    fn agent_error_display_pins_not_yet_implemented_template() {
        assert_eq!(
            AgentError::NotYetImplemented("Gemini").to_string(),
            "adapter for 'Gemini' is preview-only and not yet implemented",
        );
    }

    /// `as_db_str` produces stable snake_case identifiers for every
    /// variant. Pin the 7 strings explicitly — downstream tooling
    /// joins on the `agent_session.agent_kind` column, so a rename
    /// would break sessions persisted by older binaries.
    #[test]
    fn agent_kind_as_db_str_pins_seven_snake_case_strings() {
        for (kind, expected) in [
            (AgentKind::ClaudeCode, "claude_code"),
            (AgentKind::Cursor, "cursor"),
            (AgentKind::Codex, "codex"),
            (AgentKind::Gemini, "gemini"),
            (AgentKind::OpenCode, "opencode"),
            (AgentKind::Copilot, "copilot"),
            (AgentKind::FactoryAi, "factory_ai"),
        ] {
            assert_eq!(kind.as_db_str(), expected);
        }
    }

    /// `as_cli_slug` produces hyphenated strings (different from DB
    /// form for the two two-word agents). Pin all 7 so a rename can
    /// be reviewed at this gate.
    #[test]
    fn agent_kind_as_cli_slug_pins_seven_hyphenated_strings() {
        for (kind, expected) in [
            (AgentKind::ClaudeCode, "claude-code"),
            (AgentKind::Cursor, "cursor"),
            (AgentKind::Codex, "codex"),
            (AgentKind::Gemini, "gemini"),
            (AgentKind::OpenCode, "opencode"),
            (AgentKind::Copilot, "copilot"),
            (AgentKind::FactoryAi, "factory-ai"),
        ] {
            assert_eq!(kind.as_cli_slug(), expected);
        }
    }

    /// `from_cli_slug` accepts the documented short-form aliases:
    /// `"claude"` → ClaudeCode, `"open-code"` → OpenCode,
    /// `"factory"` → FactoryAi. Pin them all so a refactor that
    /// tightens the matcher gets caught.
    #[test]
    fn agent_kind_from_cli_slug_accepts_short_form_aliases() {
        assert_eq!(
            AgentKind::from_cli_slug("claude"),
            Some(AgentKind::ClaudeCode),
        );
        assert_eq!(
            AgentKind::from_cli_slug("open-code"),
            Some(AgentKind::OpenCode),
        );
        assert_eq!(
            AgentKind::from_cli_slug("factory"),
            Some(AgentKind::FactoryAi),
        );
    }

    /// `AgentKind::all()` returns exactly the 7 documented variants
    /// in registration order. Adding an 8th variant must force a
    /// test update.
    #[test]
    fn agent_kind_all_returns_seven_variants_in_registration_order() {
        let all = AgentKind::all();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], AgentKind::ClaudeCode);
        assert_eq!(all[6], AgentKind::FactoryAi);

        // All 7 must be distinct via HashSet.
        use std::collections::HashSet;
        let set: HashSet<AgentKind> = all.iter().copied().collect();
        assert_eq!(set.len(), 7);
    }

    /// `AgentStability` serde-serialises as snake_case — the
    /// `libra agent doctor --json` public CLI contract depends on
    /// these strings. Pin both variants.
    #[test]
    fn agent_stability_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&AgentStability::Stable).unwrap(),
            "\"stable\"",
        );
        assert_eq!(
            serde_json::to_string(&AgentStability::Preview).unwrap(),
            "\"preview\"",
        );
    }
}
