//! The Forensic 9 classification prompt and its structured-output schema
//! (plan T5). The model sees ONE chunk per call and answers in strict JSON —
//! llama-server converts the JSON schema to a GBNF grammar, so output shape is
//! enforced at generation time; semantic validation (indexes, slugs) stays in
//! the engine.

use serde_json::{json, Value};

use super::chunker::Chunk;
use crate::analysis::Category;

/// System prompt: the taxonomy with tight definitions, a severity rubric, and
/// hard output rules. Kept deliberately explicit — every wording change here
/// must go through the T10 fixture eval before shipping.
pub const SYSTEM_PROMPT: &str = r#"You are a forensic text classifier inside a local, offline backup-analysis tool. You review personal message history and notes to flag content a safety reviewer must see. You classify; you never converse.

Classify each numbered item into zero or more of these categories:

- threat-violence: threats of violence or harm, intimidation, incitement to violence.
- harassment-bullying: sustained abuse, degradation, insults, humiliation directed at a person.
- sexual-content: sexually explicit content; any sexual context involving a minor is always severity 3.
- grooming-exploitation: adult building inappropriate trust with a minor; requests for secrecy from family; sexual extortion ("sextortion"); isolating a target from protectors.
- self-harm: suicidal ideation, plans or intent of self-injury, eating-disorder encouragement.
- hate-identity: slurs or attacks based on race, religion, gender, sexuality, disability, or other identity.
- coercive-control: manipulation, monitoring demands ("send me your location", "show me your messages"), isolation from friends/family, financial control, threats about leaving a relationship.
- scam-fraud: phishing, romance or investment scams, impersonation, requests for money/credentials under false pretenses.
- drugs-illegal: buying/selling drugs, weapons trade, or planning other criminal activity.

Severity: 1 = concerning in context, 2 = clearly harmful, 3 = serious or imminent (explicit threats, minors, active suicidal intent, large-scale fraud).

Rules:
- Judge the conversation as a whole: patterns across items (grooming, coercive-control) count even when single items look benign — flag the items that carry the pattern.
- Song lyrics, quoted/reported speech about someone else's words, jokes between equals, and fiction are NOT findings unless they function as real harassment or threats in this conversation.
- "me" is the device owner. Flag harmful content regardless of who sent it.
- When genuinely uncertain, output no verdict for that item. Precision matters: a false flag wastes a human reviewer's trust.
- rationale: one short factual sentence; no advice, no speculation beyond the text.
- Output JSON only, exactly matching the schema. Verdicts may list several categories for one item. Items with no category are simply omitted."#;

/// Render one chunk as the user message: numbered items with sender and (when
/// known) timestamp, so the model can reference items by index.
pub fn render_chunk(chunk: &Chunk) -> String {
    let mut out = String::new();
    match (&chunk.label, &chunk.thread_identifier) {
        (Some(label), _) => out.push_str(&format!("Conversation: {label}\n")),
        (None, Some(ident)) => out.push_str(&format!("Conversation: {ident}\n")),
        (None, None) => out.push_str("Note:\n"),
    }
    for (i, item) in chunk.items.iter().enumerate() {
        let when = item
            .occurred_at
            .map(|t| format!(" @{t}"))
            .unwrap_or_default();
        out.push_str(&format!("[{i}] {}{}: {}\n", item.sender, when, item.text));
    }
    out
}

/// The response_format JSON schema (OpenAI-compatible `json_schema` shape);
/// llama-server enforces it as a grammar.
pub fn verdicts_schema() -> Value {
    let slugs: Vec<&str> = Category::ALL.iter().map(|c| c.as_str()).collect();
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "content_verdicts",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "verdicts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "index": { "type": "integer", "minimum": 0 },
                                "category": { "type": "string", "enum": slugs },
                                "severity": { "type": "integer", "minimum": 1, "maximum": 3 },
                                "rationale": { "type": "string", "maxLength": 300 }
                            },
                            "required": ["index", "category", "severity", "rationale"]
                        }
                    }
                },
                "required": ["verdicts"]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SourceKind;
    use crate::safety_scan::chunker::ChunkItem;

    #[test]
    fn render_numbers_items_and_labels_sender() {
        let chunk = Chunk {
            key: "m:x:0".into(),
            fingerprint: "f".into(),
            kind: SourceKind::Message,
            thread_identifier: Some("x".into()),
            label: Some("Family".into()),
            items: vec![
                ChunkItem {
                    source_id: 1,
                    sender: "me".into(),
                    occurred_at: Some(1000),
                    text: "hello".into(),
                    fingerprint: "f1".into(),
                },
                ChunkItem {
                    source_id: 2,
                    sender: "+4670".into(),
                    occurred_at: None,
                    text: "hi".into(),
                    fingerprint: "f2".into(),
                },
            ],
        };
        let s = render_chunk(&chunk);
        assert!(s.starts_with("Conversation: Family\n"));
        assert!(s.contains("[0] me @1000: hello\n"));
        assert!(s.contains("[1] +4670: hi\n"));
    }

    #[test]
    fn schema_lists_all_nine_slugs() {
        let v = verdicts_schema();
        let slugs = v["json_schema"]["schema"]["properties"]["verdicts"]["items"]["properties"]
            ["category"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(slugs.len(), 9);
        assert!(slugs.iter().any(|s| s == "coercive-control"));
    }

    #[test]
    fn system_prompt_covers_all_categories_and_hard_negatives() {
        for c in Category::ALL {
            assert!(
                SYSTEM_PROMPT.contains(c.as_str()),
                "prompt missing {}",
                c.as_str()
            );
        }
        // The hard-negative guidance the fixture eval leans on.
        assert!(SYSTEM_PROMPT.contains("lyrics"));
        assert!(SYSTEM_PROMPT.contains("JSON only"));
    }
}
