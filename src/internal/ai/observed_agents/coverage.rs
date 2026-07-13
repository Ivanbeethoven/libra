//! Coverage v1 — canonical turn form, digest, and the shared turn splitter.
//!
//! Implements `docs/development/tracing/coverage-v1.md` (plan-20260713
//! ADR-DR-08 / ADR-DR-12 / DR-05c-0). Live hook writers, import writers (M4)
//! and the OpenCode export bridge (M3) all normalize transcripts through this
//! module so the same content produces the same
//! `(logical_turn_key, coverage_digest)` on every path — the cross-path
//! idempotence the `agent_coverage_claim` gate depends on.
//!
//! Strictness notes:
//! - [`CanonValue`] parsing rejects duplicate object keys and non-integer
//!   numbers (coverage-v1.md §4.5/§4.6) instead of silently last-wins /
//!   lossy-float behavior — different producers would otherwise disagree on
//!   the digest for the same malformed source.
//! - Canonical bytes use minimal escaping, raw UTF-8 and recursive key
//!   sorting by Unicode code point (coverage-v1.md §4).

use std::{collections::BTreeMap, fmt};

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};

/// Coverage schema version this module implements (`coverage-v1.md`).
pub const COVERAGE_SCHEMA_VERSION: i64 = 1;

/// Strict JSON value for the coverage v1 domain: integers only, duplicate
/// object keys rejected at parse time, objects held sorted (BTreeMap) so the
/// canonical writer just iterates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<CanonValue>),
    Object(BTreeMap<String, CanonValue>),
}

impl CanonValue {
    pub fn get(&self, key: &str) -> Option<&CanonValue> {
        match self {
            CanonValue::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            CanonValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            CanonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[CanonValue]> {
        match self {
            CanonValue::Array(items) => Some(items),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for CanonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CanonVisitor;

        impl<'de> Visitor<'de> for CanonVisitor {
            type Value = CanonValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a coverage-v1 JSON value (integer numbers, unique keys)")
            }

            fn visit_unit<E>(self) -> Result<CanonValue, E> {
                Ok(CanonValue::Null)
            }

            fn visit_bool<E>(self, v: bool) -> Result<CanonValue, E> {
                Ok(CanonValue::Bool(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<CanonValue, E> {
                Ok(CanonValue::Int(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<CanonValue, E> {
                i64::try_from(v)
                    .map(CanonValue::Int)
                    .map_err(|_| E::custom("coverage v1 rejects integers beyond i64 range"))
            }

            fn visit_f64<E: de::Error>(self, _v: f64) -> Result<CanonValue, E> {
                Err(E::custom(
                    "coverage v1 rejects non-integer numbers in semantic positions",
                ))
            }

            fn visit_str<E>(self, v: &str) -> Result<CanonValue, E> {
                Ok(CanonValue::Str(v.to_string()))
            }

            fn visit_string<E>(self, v: String) -> Result<CanonValue, E> {
                Ok(CanonValue::Str(v))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<CanonValue, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<CanonValue>()? {
                    items.push(item);
                }
                Ok(CanonValue::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<CanonValue, A::Error> {
                let mut object = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, CanonValue>()? {
                    if object.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!(
                            "coverage v1 rejects duplicate object key '{key}'"
                        )));
                    }
                }
                Ok(CanonValue::Object(object))
            }
        }

        deserializer.deserialize_any(CanonVisitor)
    }
}

/// Parse a strict coverage-v1 value from raw JSON bytes.
pub fn parse_canon_value(bytes: &[u8]) -> Result<CanonValue, serde_json::Error> {
    serde_json::from_slice(bytes)
}

/// Append the canonical (minimal-escape) JSON string form of `s` to `out`
/// (coverage-v1.md §4.4): only `"`, `\` and control chars < U+0020 escape;
/// two-char forms where defined, lowercase `\u00xx` otherwise; raw UTF-8 for
/// everything else.
fn write_canon_string(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{0008}' => out.extend_from_slice(b"\\b"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\u{000C}' => out.extend_from_slice(b"\\f"),
            '\r' => out.extend_from_slice(b"\\r"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

fn write_canon_value(out: &mut Vec<u8>, value: &CanonValue) {
    match value {
        CanonValue::Null => out.extend_from_slice(b"null"),
        CanonValue::Bool(true) => out.extend_from_slice(b"true"),
        CanonValue::Bool(false) => out.extend_from_slice(b"false"),
        CanonValue::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        CanonValue::Str(s) => write_canon_string(out, s),
        CanonValue::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canon_value(out, item);
            }
            out.push(b']');
        }
        CanonValue::Object(map) => {
            out.push(b'{');
            for (i, (key, value)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canon_string(out, key);
                out.push(b':');
                write_canon_value(out, value);
            }
            out.push(b'}');
        }
    }
}

/// One semantic record of a normalized turn (coverage-v1.md §3). Exactly the
/// digest allowlist — provenance (model/usage/timestamps/paths) never appears
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticRecord {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    ToolCall {
        call_id: Option<String>,
        input: CanonValue,
        name: String,
    },
    ToolResult {
        call_id: Option<String>,
        content: String,
        is_error: bool,
    },
}

fn write_opt_string(out: &mut Vec<u8>, value: &Option<String>) {
    match value {
        Some(s) => write_canon_string(out, s),
        None => out.extend_from_slice(b"null"),
    }
}

impl SemanticRecord {
    /// Canonical serialization of one record. Key order is the sorted order
    /// pinned by coverage-v1.md §3 for each shape.
    fn write_canonical(&self, out: &mut Vec<u8>) {
        match self {
            SemanticRecord::User { text } => {
                out.extend_from_slice(b"{\"role\":\"user\",\"text\":");
                write_canon_string(out, text);
                out.push(b'}');
            }
            SemanticRecord::Assistant { text } => {
                out.extend_from_slice(b"{\"role\":\"assistant\",\"text\":");
                write_canon_string(out, text);
                out.push(b'}');
            }
            SemanticRecord::ToolCall {
                call_id,
                input,
                name,
            } => {
                out.extend_from_slice(b"{\"call_id\":");
                write_opt_string(out, call_id);
                out.extend_from_slice(b",\"input\":");
                write_canon_value(out, input);
                out.extend_from_slice(b",\"name\":");
                write_canon_string(out, name);
                out.extend_from_slice(b",\"role\":\"tool_call\"}");
            }
            SemanticRecord::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                out.extend_from_slice(b"{\"call_id\":");
                write_opt_string(out, call_id);
                out.extend_from_slice(b",\"content\":");
                write_canon_string(out, content);
                out.extend_from_slice(b",\"is_error\":");
                out.extend_from_slice(if *is_error { b"true" } else { b"false" });
                out.extend_from_slice(b",\"role\":\"tool_result\"}");
            }
        }
    }
}

/// Canonical bytes of one turn: the JSON array of its semantic records
/// (coverage-v1.md §4).
pub fn canonical_turn_bytes(records: &[SemanticRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'[');
    for (i, record) in records.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        record.write_canonical(&mut out);
    }
    out.push(b']');
    out
}

/// `coverage_digest`: lowercase-hex SHA-256 of the canonical turn bytes.
pub fn coverage_digest_hex(records: &[SemanticRecord]) -> String {
    hex::encode(Sha256::digest(canonical_turn_bytes(records)))
}

/// Turn completeness (coverage-v1.md §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    Incomplete,
    Complete,
}

impl Completeness {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Completeness::Incomplete => "incomplete",
            Completeness::Complete => "complete",
        }
    }
}

/// One normalized logical turn (coverage-v1.md §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedTurn {
    pub logical_turn_key: String,
    pub ordinal: usize,
    pub completeness: Completeness,
    pub records: Vec<SemanticRecord>,
}

impl NormalizedTurn {
    pub fn digest_hex(&self) -> String {
        coverage_digest_hex(&self.records)
    }
}

/// Validate a provider-supplied turn/message id for use as a
/// `logical_turn_key` (coverage-v1.md §2): non-empty, ≤ 64 chars, restricted
/// alphabet. Anything else falls back to the ordinal key.
fn valid_provider_turn_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn ordinal_key(ordinal: usize) -> String {
    format!("ordinal:{ordinal}")
}

/// Extract plain text out of a strict message `content` value (string form or
/// the block-array form with `{"type":"text","text":…}` entries), joining
/// multiple text blocks with `\n` in source order (coverage-v1.md §3).
fn canon_content_text(content: &CanonValue) -> String {
    match content {
        CanonValue::Str(text) => text.clone(),
        CanonValue::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(CanonValue::as_str) == Some("text") {
                    block.get("text").and_then(CanonValue::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Does this user-line content carry actual human input (a string body or at
/// least one `text` block), as opposed to being purely tool_result plumbing?
fn user_content_has_human_text(content: &CanonValue) -> bool {
    match content {
        CanonValue::Str(_) => true,
        CanonValue::Array(blocks) => blocks
            .iter()
            .any(|block| block.get("type").and_then(CanonValue::as_str) == Some("text")),
        _ => false,
    }
}

/// Split a Claude Code session JSONL transcript into normalized turns.
///
/// Line shapes follow the live envelope observed by `extract_claude_code`:
/// `{"type":"user"|"assistant","uuid":…,"message":{"role":…,"content":…}}`.
/// A user line with human text starts a new logical turn; user lines carrying
/// only `tool_result` blocks attach to the current turn (they answer the
/// assistant's tool calls — coverage-v1.md §2). Unparseable or duplicate-key
/// lines mark the enclosing turn `incomplete` and are skipped; they never
/// contribute partial content to a digest.
pub fn normalize_claude_transcript(data: &[u8]) -> Vec<NormalizedTurn> {
    let mut turns: Vec<NormalizedTurn> = Vec::new();

    fn open_turn(turns: &mut Vec<NormalizedTurn>, key: Option<String>) -> &mut NormalizedTurn {
        let ordinal = turns.len();
        let logical_turn_key = key
            .filter(|id| valid_provider_turn_id(id))
            .unwrap_or_else(|| ordinal_key(ordinal));
        turns.push(NormalizedTurn {
            logical_turn_key,
            ordinal,
            completeness: Completeness::Complete,
            records: Vec::new(),
        });
        turns.last_mut().expect("just pushed") // INVARIANT: push above
    }

    fn current_or_open(turns: &mut Vec<NormalizedTurn>) -> &mut NormalizedTurn {
        if turns.is_empty() {
            return open_turn(turns, None);
        }
        turns.last_mut().expect("non-empty checked") // INVARIANT: checked above
    }

    for line in data.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry = match parse_canon_value(line) {
            Ok(value) => value,
            Err(_) => {
                // Corrupt / truncated / duplicate-key line: poison the
                // enclosing turn instead of guessing at its content.
                current_or_open(&mut turns).completeness = Completeness::Incomplete;
                continue;
            }
        };
        let entry_type = entry.get("type").and_then(CanonValue::as_str).unwrap_or("");
        let uuid = entry
            .get("uuid")
            .and_then(CanonValue::as_str)
            .map(str::to_string);
        let Some(message) = entry.get("message") else {
            continue; // metadata/system line — not semantic content
        };
        let Some(content) = message.get("content") else {
            continue;
        };
        match entry_type {
            "user" => {
                if user_content_has_human_text(content) {
                    let turn = open_turn(&mut turns, uuid);
                    turn.records.push(SemanticRecord::User {
                        text: canon_content_text(content),
                    });
                    // Rare but possible: the same user line also carries
                    // tool_result blocks; fall through to collect them below.
                }
                if let CanonValue::Array(blocks) = content {
                    for block in blocks {
                        if block.get("type").and_then(CanonValue::as_str) == Some("tool_result") {
                            let call_id = block
                                .get("tool_use_id")
                                .and_then(CanonValue::as_str)
                                .map(str::to_string);
                            let result_content = block
                                .get("content")
                                .map(canon_content_text)
                                .unwrap_or_default();
                            let is_error =
                                block.get("is_error").and_then(CanonValue::as_bool) == Some(true);
                            current_or_open(&mut turns)
                                .records
                                .push(SemanticRecord::ToolResult {
                                    call_id,
                                    content: result_content,
                                    is_error,
                                });
                        }
                    }
                }
            }
            "assistant" => {
                let turn = current_or_open(&mut turns);
                let text = canon_content_text(content);
                if !text.is_empty() {
                    turn.records.push(SemanticRecord::Assistant { text });
                }
                if let CanonValue::Array(blocks) = content {
                    for block in blocks {
                        if block.get("type").and_then(CanonValue::as_str) == Some("tool_use") {
                            let name = block
                                .get("name")
                                .and_then(CanonValue::as_str)
                                .unwrap_or("")
                                .to_string();
                            let call_id = block
                                .get("id")
                                .and_then(CanonValue::as_str)
                                .map(str::to_string);
                            let input = block.get("input").cloned().unwrap_or(CanonValue::Null);
                            turn.records.push(SemanticRecord::ToolCall {
                                call_id,
                                input,
                                name,
                            });
                        }
                    }
                }
            }
            _ => {} // summary / system / other line kinds: not semantic
        }
    }

    // Drop turns that ended up with no semantic records (e.g. a poisoned
    // fragment before the first real turn) unless they were poisoned — a
    // poisoned empty turn still matters as evidence of unreadable content.
    turns.retain(|turn| !turn.records.is_empty() || turn.completeness == Completeness::Incomplete);
    // Re-number ordinals (and ordinal-derived keys) after the retain so the
    // ordinal fallback stays gap-free.
    for (i, turn) in turns.iter_mut().enumerate() {
        if turn.ordinal != i {
            if turn.logical_turn_key == ordinal_key(turn.ordinal) {
                turn.logical_turn_key = ordinal_key(i);
            }
            turn.ordinal = i;
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_hi_assistant_hello() -> Vec<SemanticRecord> {
        vec![
            SemanticRecord::User {
                text: "hi".to_string(),
            },
            SemanticRecord::Assistant {
                text: "hello".to_string(),
            },
        ]
    }

    /// coverage-v1.md §5 vector 1 — canonical bytes and digest are normative.
    #[test]
    fn golden_vector_1_user_assistant() {
        let records = user_hi_assistant_hello();
        assert_eq!(
            canonical_turn_bytes(&records),
            br#"[{"role":"user","text":"hi"},{"role":"assistant","text":"hello"}]"#.to_vec()
        );
        assert_eq!(
            coverage_digest_hex(&records),
            "df991cd9a1ac5c12c32b8cdf0254c3dfbbf26485b505a5afc83a90d1128ebc54"
        );
    }

    /// coverage-v1.md §5 vector 2 — unsorted source keys canonicalize sorted.
    #[test]
    fn golden_vector_2_tool_call_and_result() {
        let input = parse_canon_value(br#"{"b":2,"a":"x"}"#).expect("strict parse");
        let records = vec![
            SemanticRecord::ToolCall {
                call_id: Some("c1".to_string()),
                input,
                name: "grep".to_string(),
            },
            SemanticRecord::ToolResult {
                call_id: Some("c1".to_string()),
                content: "2 matches".to_string(),
                is_error: false,
            },
        ];
        assert_eq!(
            canonical_turn_bytes(&records),
            br#"[{"call_id":"c1","input":{"a":"x","b":2},"name":"grep","role":"tool_call"},{"call_id":"c1","content":"2 matches","is_error":false,"role":"tool_result"}]"#.to_vec()
        );
        assert_eq!(
            coverage_digest_hex(&records),
            "af085109b1584fd77bb3b752c1141c1c0f0f12b51a163e94b0f91ddd329460a4"
        );
    }

    /// coverage-v1.md §5 vector 3 — escapes and raw UTF-8.
    #[test]
    fn golden_vector_3_escapes_and_unicode() {
        let records = vec![SemanticRecord::User {
            text: "line1\nline2 \"quoted\" \\ 中文".to_string(),
        }];
        assert_eq!(
            canonical_turn_bytes(&records),
            "[{\"role\":\"user\",\"text\":\"line1\\nline2 \\\"quoted\\\" \\\\ 中文\"}]".as_bytes()
        );
        assert_eq!(
            coverage_digest_hex(&records),
            "f1e76bf75df5d6b0f67a46806abb256cd6de30eaea30e45254a8a66cf5183356"
        );
    }

    #[test]
    fn canon_parse_rejects_duplicate_keys() {
        assert!(parse_canon_value(br#"{"a":1,"a":2}"#).is_err());
    }

    #[test]
    fn canon_parse_rejects_floats() {
        assert!(parse_canon_value(br#"{"a":1.5}"#).is_err());
        assert!(parse_canon_value(br#"{"a":18446744073709551615}"#).is_err());
    }

    #[test]
    fn claude_splitter_groups_turns_and_attaches_tool_results() {
        let transcript = [
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"run grep"}}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"role":"assistant","content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"c1","name":"grep","input":{"b":2,"a":"x"}}]}}"#,
            r#"{"type":"user","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"2 matches"}]}}"#,
            r#"{"type":"user","uuid":"u3","message":{"role":"user","content":"thanks"}}"#,
            r#"{"type":"assistant","uuid":"a2","message":{"role":"assistant","content":[{"type":"text","text":"any time"}]}}"#,
        ]
        .join("\n");
        let turns = normalize_claude_transcript(transcript.as_bytes());
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].logical_turn_key, "u1");
        assert_eq!(turns[0].ordinal, 0);
        assert_eq!(turns[0].completeness, Completeness::Complete);
        assert_eq!(turns[0].records.len(), 4); // user, assistant text, tool call, tool result
        assert!(matches!(
            &turns[0].records[3],
            SemanticRecord::ToolResult { call_id: Some(id), content, is_error: false }
                if id == "c1" && content == "2 matches"
        ));
        assert_eq!(turns[1].logical_turn_key, "u3");
        assert_eq!(turns[1].ordinal, 1);
        assert_eq!(turns[1].records.len(), 2);
    }

    #[test]
    fn claude_splitter_same_content_same_digest_across_parses() {
        let transcript = concat!(
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
        );
        let a = normalize_claude_transcript(transcript.as_bytes());
        let b = normalize_claude_transcript(transcript.as_bytes());
        assert_eq!(a[0].digest_hex(), b[0].digest_hex());
        // And it matches golden vector 1 (same semantic content).
        assert_eq!(
            a[0].digest_hex(),
            "df991cd9a1ac5c12c32b8cdf0254c3dfbbf26485b505a5afc83a90d1128ebc54"
        );
    }

    #[test]
    fn claude_splitter_marks_corrupt_tail_incomplete() {
        let transcript = concat!(
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","message":{"role":"assistant","content":[{"type":"te"#, // truncated
        );
        let turns = normalize_claude_transcript(transcript.as_bytes());
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].completeness, Completeness::Incomplete);
    }

    #[test]
    fn claude_splitter_invalid_uuid_falls_back_to_ordinal() {
        let transcript =
            r#"{"type":"user","uuid":"has spaces !","message":{"role":"user","content":"hi"}}"#;
        let turns = normalize_claude_transcript(transcript.as_bytes());
        assert_eq!(turns[0].logical_turn_key, "ordinal:0");
    }

    #[test]
    fn truncated_and_complete_same_turn_share_logical_key_but_differ_in_digest() {
        // The whole point of ADR-DR-08: same logical key, different content
        // revision — never two separate turns.
        let truncated =
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"hi"}}"#.to_string();
        let complete = format!(
            "{truncated}\n{}",
            r#"{"type":"assistant","uuid":"a1","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#
        );
        let t = normalize_claude_transcript(truncated.as_bytes());
        let c = normalize_claude_transcript(complete.as_bytes());
        assert_eq!(t[0].logical_turn_key, c[0].logical_turn_key);
        assert_ne!(t[0].digest_hex(), c[0].digest_hex());
    }
}
