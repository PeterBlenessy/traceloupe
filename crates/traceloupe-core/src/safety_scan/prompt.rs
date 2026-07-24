//! The Forensic 9 classification prompt and its structured-output grammar
//! (plan T5). The model sees ONE chunk per call and answers in strict JSON,
//! constrained by a hand-written GBNF grammar ([`verdicts_grammar`]) passed to
//! llama-server's `grammar` field, so output shape is enforced at generation
//! time; semantic validation (indexes, slugs) stays in the engine.

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

/// A raw GBNF grammar (llama-server `grammar` field) constraining the verdicts
/// output. We hand-write GBNF rather than pass a JSON schema via `response_format`
/// because of two behaviours verified empirically against the pinned server
/// (b10075) with real E2B/E4B calls — see `docs/research/safety-scan-grammar.md`:
///
///  1. **`maxItems` is NOT enforced** on the `response_format`/`json_schema`
///     path. Without a real bound the model keeps appending array elements until
///     it hits the token cap, truncating the JSON mid-element into unparseable
///     output — the chunk is then skipped (observed ~15–45% of chunks failing;
///     a controlled probe hit the token cap with the array still open every
///     time). GBNF bounded repetition `(...){0,n-1}` *is* enforced, so the array
///     closes deterministically and the output stays valid and short.
///
///  2. **Whitespace must be present but bounded.** Forbidding inter-token
///     whitespace entirely (compact JSON) collapses the weak sweep tier to an
///     empty array — it stops detecting harmful content at all. Allowing the
///     model its natural pretty-print whitespace restores detection; leaving it
///     *unbounded* (`ws ::= [ \t\n]*`) lets the model loop on newlines until the
///     token cap. `ws ::= [ \t\n]{0,4}` gives it room without a loop.
///
/// `max_items` bounds the array (one verdict per chunk item is the norm; extra
/// categories are rare). At least 1, so single-item chunks still parse.
pub fn verdicts_grammar(max_items: usize) -> String {
    let m = max_items.max(1);
    let cat_alt = Category::ALL
        .iter()
        .map(|c| format!("\"\\\"{}\\\"\"", c.as_str()))
        .collect::<Vec<_>>()
        .join(" | ");
    // A single-item chunk allows exactly one verdict (no repetition suffix);
    // otherwise 1..=m via `verdict (ws "," ws verdict){0,m-1}`.
    let rep = if m <= 1 {
        String::new()
    } else {
        format!("(ws \",\" ws verdict){{0,{}}}", m - 1)
    };
    const TEMPLATE: &str = r##"root ::= "{" ws "\"verdicts\"" ws ":" ws "[" ws items? ws "]" ws "}"
items ::= verdict __REP__
verdict ::= "{" ws "\"index\"" ws ":" ws index ws "," ws "\"category\"" ws ":" ws category ws "," ws "\"severity\"" ws ":" ws severity ws "," ws "\"rationale\"" ws ":" ws rationale ws "}"
category ::= __CAT__
severity ::= "1" | "2" | "3"
index ::= [0-9] | [1-9] [0-9] [0-9]?
rationale ::= "\"" char{«redacted»} "\""
char ::= [^"\\\x00-\x1F] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F])
ws ::= [ \t\n]{0,4}"##;
    TEMPLATE
        .replace("__REP__", &rep)
        .replace("__CAT__", &cat_alt)
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
    fn grammar_lists_all_nine_slugs_and_bounds_the_array() {
        let g = verdicts_grammar(25);
        // Every category slug is an allowed literal in the `category` rule.
        for c in Category::ALL {
            assert!(
                g.contains(&format!("\"\\\"{}\\\"\"", c.as_str())),
                "grammar missing slug {}",
                c.as_str()
            );
        }
        // The array is bounded (repetition capped at max_items-1) and whitespace
        // is bounded (present, so detection survives; capped, so it can't loop).
        assert!(
            g.contains("(ws \",\" ws verdict){0,24}"),
            "array not bounded"
        );
        assert!(
            g.contains("ws ::= [ \\t\\n]{0,4}"),
            "whitespace not bounded"
        );
        assert!(!g.contains("ws ::= [ \\t\\n]*"), "unbounded ws would loop");
    }

    #[test]
    fn grammar_single_item_allows_exactly_one_verdict() {
        // A 1-item chunk drops the repetition suffix: `items ::= verdict`.
        let g = verdicts_grammar(1);
        assert!(g.contains("items ::= verdict \n") || g.contains("items ::= verdict\n"));
        assert!(
            !g.contains("{0,0}"),
            "empty repetition range is invalid GBNF"
        );
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
