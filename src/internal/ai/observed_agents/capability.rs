//! Capability declaration contract for observed external agents (AG-16 / E1).
//!
//! This module freezes the Libra side of entire's `DeclaredCaps` wire
//! contract: an 8-bool capability set ([`DeclaredAgentCaps`]) plus the
//! optional capability traits an adapter may implement. The serialized
//! snake_case keys are pinned by `compat_agent_capability_matrix_pin` —
//! renaming a field is a public wire break.
//!
//! Design notes (see `docs/development/tracing/agent.md` E1):
//!
//! - `ModelExtractor`, `SkillEventExtractor` and session-base-dir style
//!   capabilities are deliberately **not** part of the 8-bool set. External
//!   `libra-agent-*` binaries unlock them via the v1 `capabilities.methods[]`
//!   negotiation (AG-18); built-in adapters unlock them via trait impls.
//! - `PromptExtractor` has no key of its own — its gate reuses
//!   `transcript_analyzer`.
//! - Built-in adapters do **not** implement [`CapabilityDeclarer`]; they rely
//!   on the `ObservedAgent::declared_capabilities()` introspection default.
//!   Only external RPC shims (AG-18) override the introspection with an
//!   explicit declaration.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::adapter::{AgentSessionCtx, ObservedAgent};
use crate::internal::ai::completion::CompletionUsageSummary;

/// The frozen 8-bool capability wire contract (E1).
///
/// Serialized keys are snake_case and must match E1 exactly:
/// `hooks`, `transcript_analyzer`, `transcript_preparer`, `token_calculator`,
/// `compact_transcript`, `text_generator`, `hook_response_writer`,
/// `subagent_aware_extractor` — no more, no fewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DeclaredAgentCaps {
    pub hooks: bool,
    pub transcript_analyzer: bool,
    pub transcript_preparer: bool,
    pub token_calculator: bool,
    pub compact_transcript: bool,
    pub text_generator: bool,
    pub hook_response_writer: bool,
    pub subagent_aware_extractor: bool,
}

/// Explicit capability declaration for external `libra-agent-*` shims.
///
/// External binaries cannot be introspected via trait impls, so their RPC
/// shim (AG-18) implements this trait from the negotiated `info` payload and
/// overrides `ObservedAgent::declared_capabilities()`. Built-in adapters must
/// NOT implement this — their capabilities derive from `as_*` accessors.
pub trait CapabilityDeclarer {
    fn declared_capabilities(&self) -> DeclaredAgentCaps;
}

/// Optional capability: transcript position tracking and modified-file
/// extraction (E1 `transcript_analyzer`). Implementations land in AG-21.
pub trait TranscriptAnalyzer: ObservedAgent {
    /// Current logical position (byte offset) of the analyzed transcript.
    fn transcript_position(&self, data: &[u8]) -> Result<usize>;
    /// Files the agent reported as modified, starting at `from_offset`.
    fn extract_modified_files_from_offset(
        &self,
        data: &[u8],
        from_offset: usize,
    ) -> Result<Vec<PathBuf>>;
}

/// Optional capability: user-prompt extraction. No 8-bool key of its own —
/// for external agents the gate reuses `transcript_analyzer` (E1).
pub trait PromptExtractor: ObservedAgent {
    fn extract_prompts(&self, data: &[u8], from_offset: usize) -> Result<Vec<String>>;
}

/// Optional capability: pre-persist transcript preparation (E1
/// `transcript_preparer`), e.g. flushing provider-side buffers.
pub trait TranscriptPreparer: ObservedAgent {
    fn prepare_transcript(&self, session: &AgentSessionCtx) -> Result<()>;
}

/// Optional capability: token usage extraction (E1 `token_calculator`).
///
/// E6 wire keys map explicitly onto [`CompletionUsageSummary`] in AG-21;
/// the trait only fixes the call shape.
pub trait TokenCalculator: ObservedAgent {
    fn calculate_token_usage(
        &self,
        data: &[u8],
        from_offset: usize,
    ) -> Result<CompletionUsageSummary>;
}

/// Optional capability: model-id extraction. Deliberately NOT part of the
/// 8-bool set — external agents unlock it via the v1 `methods[]` negotiation
/// (`model_extract`), built-ins via this trait impl.
pub trait ModelExtractor: ObservedAgent {
    fn extract_model(&self, data: &[u8]) -> Result<Option<String>>;
}

/// Optional capability: provider-backed text generation (E1 `text_generator`).
pub trait TextGenerator: ObservedAgent {
    fn generate_text(&self, prompt: &str, model: Option<&str>) -> Result<String>;
}

/// Optional capability: transcript compaction (E1 `compact_transcript`).
pub trait TranscriptCompactor: ObservedAgent {
    fn compact_transcript(&self, data: &[u8]) -> Result<Vec<u8>>;
}

/// Optional capability: writing a response back through the agent's hook
/// channel (E1 `hook_response_writer`).
pub trait HookResponseWriter: ObservedAgent {
    fn write_hook_response(&self, message: &str) -> Result<()>;
}

/// Optional capability: subagent-aware aggregate extraction (E1
/// `subagent_aware_extractor`) — modified files and token totals that
/// include nested subagent activity.
pub trait SubagentAwareExtractor: ObservedAgent {
    fn extract_all_modified_files(&self, data: &[u8]) -> Result<Vec<PathBuf>>;
    fn total_token_usage_including_subagents(&self, data: &[u8]) -> Result<CompletionUsageSummary>;
}

/// Optional capability: skill-event projection (E7). Deliberately NOT part
/// of the 8-bool set — external agents unlock it via the v1 `methods[]`
/// negotiation (`skill_events`), built-ins via this trait impl.
pub trait SkillEventExtractor: ObservedAgent {
    fn extract_skill_events(&self, data: &[u8], from_offset: usize) -> Result<Vec<SkillEvent>>;
}

/// How a skill invocation was observed in the transcript (E7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillEventType {
    PromptInvocation,
    ToolInvocation,
}

/// The signal that produced a skill event (E7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillEventSignal {
    InputSlashCommand,
    PromptSlashCommand,
    SkillToolUse,
}

/// The skill referenced by a [`SkillEvent`] (E7 `Skill{Name}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRef {
    pub name: String,
}

/// Provenance of a [`SkillEvent`] (E7 `Source{Agent,Signal,Confidence}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillEventSource {
    /// CLI slug of the agent the event was observed from.
    pub agent: String,
    pub signal: SkillEventSignal,
    /// Extraction confidence in `[0.0, 1.0]`.
    pub confidence: f64,
}

/// One observed skill invocation projected from a transcript (E7).
///
/// The curated per-agent skill registry and the extraction pipeline land in
/// AG-21; AG-16 only freezes the wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillEvent {
    pub id: String,
    pub event_type: SkillEventType,
    pub skill: SkillRef,
    pub source: SkillEventSource,
    pub turn_id: String,
    /// RFC3339 timestamp of the observation.
    pub timestamp: String,
    /// Opaque anchor into the source transcript (e.g. a byte offset or
    /// event id), when the extractor can provide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_anchor: Option<String>,
    /// Whether the agent reported the skill natively (vs. inferred).
    pub native: bool,
    /// Whether consecutive identical events should collapse into one.
    pub collapse: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E1 pin: serialization yields exactly the 8 frozen snake_case keys.
    /// The compat guard (`compat_agent_capability_matrix_pin`) re-asserts
    /// this from outside the crate; this unit test keeps the failure local.
    #[test]
    fn declared_agent_caps_serializes_exactly_eight_e1_keys() {
        let value = serde_json::to_value(DeclaredAgentCaps::default()).unwrap();
        let object = value.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = [
            "hooks",
            "transcript_analyzer",
            "transcript_preparer",
            "token_calculator",
            "compact_transcript",
            "text_generator",
            "hook_response_writer",
            "subagent_aware_extractor",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }

    #[test]
    fn declared_agent_caps_default_is_all_false() {
        let caps = DeclaredAgentCaps::default();
        let value = serde_json::to_value(caps).unwrap();
        for (key, entry) in value.as_object().unwrap() {
            assert_eq!(entry, &serde_json::Value::Bool(false), "key {key}");
        }
    }

    #[test]
    fn skill_event_wire_shape_uses_snake_case_keys() {
        let event = SkillEvent {
            id: "evt-1".into(),
            event_type: SkillEventType::PromptInvocation,
            skill: SkillRef {
                name: "/review".into(),
            },
            source: SkillEventSource {
                agent: "claude-code".into(),
                signal: SkillEventSignal::InputSlashCommand,
                confidence: 1.0,
            },
            turn_id: "turn-1".into(),
            timestamp: "2026-07-04T00:00:00Z".into(),
            transcript_anchor: None,
            native: true,
            collapse: false,
        };
        let value = serde_json::to_value(&event).unwrap();
        let object = value.as_object().unwrap();
        for key in [
            "id",
            "event_type",
            "skill",
            "source",
            "turn_id",
            "timestamp",
            "native",
            "collapse",
        ] {
            assert!(object.contains_key(key), "missing key {key}");
        }
        assert_eq!(object["event_type"], "prompt_invocation");
        assert_eq!(object["source"]["signal"], "input_slash_command");
        // Optional anchor is omitted when absent.
        assert!(!object.contains_key("transcript_anchor"));
    }
}
