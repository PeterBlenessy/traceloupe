//! Llama Guard as the triage CONFIRMER (#474) — the second opinion the
//! Balanced/Precise scan modes promise (journey §5.5, §10.5).
//!
//! Three measured facts shape this module:
//!
//! 1. **Guard confirms focused findings well and batch findings terribly.** As
//!    a batch-mode confirmer it deleted 53% of real findings; over focused-mode
//!    findings it keeps 88% of real and removes 88% of false (§10.5). It is
//!    only ever called on one judged message with its window as context.
//! 2. **The Forensic 9 REPLACE Guard's default categories.** Strategy B
//!    (replace) beat both Meta's S1–S14 verbatim and S1–S14-plus-ours on
//!    detection AND right-category (§10.5) — nine purpose-written categories
//!    beat fourteen trained ones. The category text below mirrors the
//!    production taxonomy (`prompt::SYSTEM_PROMPT`), including its NOT
//!    exclusions as Guard "Can" lines.
//! 3. **The GGUF ships no Guard chat template.** The very first Guard run
//!    scored recall 0.00 because `/v1/chat/completions` applied a generic
//!    template and the model *continued the conversation* (§10.6 incident 1).
//!    The prompt is hand-rolled here, byte-compatible with the structure the
//!    oracle validated (tools/validate-triage-pipeline.py, stage 3), and sent
//!    to `/completion` — never the chat endpoint.
//!
//! A confirm is a KEEP/DROP gate, not a re-classification: the finding is kept
//! when Guard says "unsafe" under any category. Filtering on Guard's category
//! matching the verdict's would be a form of category narrowing, which was
//! measured harmful (§5.5).

use crate::safety_scan::client::LlmClient;
use crate::safety_scan::triage::FocusWindow;
use crate::{Error, Result};

/// The Forensic 9 rendered as Guard's unsafe-content categories. "Should not"
/// carries the category definition; "Can" carries the taxonomy's NOT
/// exclusions — the hard negatives the pipeline must not confirm.
const CATEGORIES: [(&str, &str); 9] = [
    (
        "Threats and Violence",
        "Should not\n - Threaten violence or harm against a person, intimidate, or incite violence.\nCan\n - Discuss violence in fiction, lyrics, news, history, true crime or sport.\n - Quote a threat that was made TO the speaker.",
    ),
    (
        "Harassment and Bullying",
        "Should not\n - Sustain abuse, degradation, insults or humiliation directed at a person.\nCan\n - Trade affectionate insults between friends who are equals.\n - Talk competitive trash about a game or match.\n - Describe harassment the speaker received.",
    ),
    (
        "Sexual Content",
        "Should not\n - Contain sexually explicit content; any sexual context involving a minor is always unsafe.\nCan\n - Discuss sex or sexual health clinically or medically.\n - Refer to a couple's relationship without explicit content.",
    ),
    (
        "Grooming and Exploitation",
        "Should not\n - Build inappropriate trust between an adult and a minor, request secrecy from family, sexually extort, or isolate a target from protectors.\nCan\n - Show an adult caring for a child openly, with nothing hidden from the child's family.",
    ),
    (
        "Self-Harm",
        "Should not\n - Express suicidal ideation, plans or intent of self-injury, or encourage an eating disorder.\nCan\n - Discuss recovery, or support someone else through recovery.\n - Discuss self-harm clinically or in the third person.\n - Express ordinary sadness, grief, exhaustion or stress.",
    ),
    (
        "Hate and Identity Attacks",
        "Should not\n - Use slurs or attack a person based on race, religion, gender, sexuality, disability, or other identity.\nCan\n - Discuss prejudice as a subject.\n - Quote hateful words in order to report or condemn them.",
    ),
    (
        "Coercive Control",
        "Should not\n - Manipulate, demand monitoring (location, messages, passwords), isolate from friends or family, control finances, or threaten over leaving a relationship.\nCan\n - Show a parent setting ordinary rules for a child, like a curfew.\n - Show partners sharing plans or locations voluntarily out of care.\n - Show a person naming their own jealousy and taking responsibility.\n - Describe control the speaker survived.",
    ),
    (
        "Scams and Fraud",
        "Should not\n - Phish, run romance or investment scams, impersonate, or request money or credentials under false pretenses.\nCan\n - Warn someone about a scam or explain how to avoid one.\n - Discuss ordinary payments, invoices or money between people who know each other.",
    ),
    (
        "Drugs and Illegal Activity",
        "Should not\n - Buy or sell drugs, trade weapons, or plan other criminal activity.\nCan\n - Discuss prescribed medication and its dosage.\n - Use drug words as slang praise.",
    ),
];

/// Guard answers "safe"/"unsafe" plus at most a category line; 24 tokens is
/// what the oracle's validated stage used.
pub const GUARD_MAX_TOKENS: u32 = 24;

/// Render the hand-rolled Llama Guard 3 `/completion` prompt for one focused
/// finding: the window as a conversation, the judged message repeated as the
/// final turn, and the assessment asked for ONLY that last turn — the exact
/// structure the oracle validated. Turn roles alternate by window position
/// (the oracle's mapping); who is "User" vs "Agent" is immaterial to a
/// last-message safety check, and the validated shape is kept verbatim rather
/// than improved untested.
pub fn render_confirm_prompt(window: &FocusWindow) -> String {
    let block = CATEGORIES
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| format!("S{}: {}.\n{}", i + 1, name, desc))
        .collect::<Vec<_>>()
        .join("\n");
    let mut turns: Vec<String> = window
        .items
        .iter()
        .enumerate()
        .map(|(i, m)| format!("{}: {}", if i % 2 == 0 { "User" } else { "Agent" }, m.text))
        .collect();
    turns.push(format!("Agent: {}", window.items[window.focus].text));
    format!(
        "<|begin_of_text|><|start_header_id|>user<|end_header_id|>\n\nTask: Check if there is \
         unsafe content in 'Agent' messages in conversations according our safety policy with the \
         below categories.\n\n<BEGIN UNSAFE CONTENT CATEGORIES>\n{block}\n<END UNSAFE CONTENT \
         CATEGORIES>\n\n<BEGIN CONVERSATION>\n\n{turns}\n\n<END CONVERSATION>\n\n\
         Provide your safety assessment for ONLY THE LAST Agent message:\n - First line 'safe' or \
         'unsafe'.\n - If unsafe, a second line lists the categories.<|eot_id|>\
         <|start_header_id|>assistant<|end_header_id|>\n\n",
        turns = turns.join("\n\n"),
    )
}

/// One confirmation through the PRODUCTION Guard path: the validated prompt,
/// the raw `/completion` endpoint, strict parse. This is the `confirm` closure
/// the command layer hands `run_triage` — kept here so no caller can drift
/// into a chat-templated Guard call (the §10.6 recall-0.00 mistake).
pub fn confirm_focused(client: &LlmClient, window: &FocusWindow) -> Result<bool> {
    let reply = client.complete(&render_confirm_prompt(window), GUARD_MAX_TOKENS)?;
    parse_confirm_reply(&reply).ok_or_else(|| {
        Error::Inference(
            "the confirmer answered neither 'safe' nor 'unsafe' — suspect the harness \
             (template/endpoint), not the finding"
                .into(),
        )
    })
}

/// Parse Guard's reply into KEEP (true) / DROP (false). Guard's contract is
/// "first line 'safe' or 'unsafe'"; anything else — an echo, an empty string,
/// a continuation — is a malfunctioning confirmer, and the caller must surface
/// it rather than guess (a reply that parses as neither must never silently
/// drop a real finding OR silently confirm a false one).
pub fn parse_confirm_reply(reply: &str) -> Option<bool> {
    let first = reply.trim().lines().next()?.trim().to_ascii_lowercase();
    if first.starts_with("unsafe") {
        Some(true)
    } else if first.starts_with("safe") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety_scan::triage::CensusInput;

    fn window() -> FocusWindow {
        FocusWindow {
            items: vec![
                CensusInput {
                    source_id: 1,
                    thread_identifier: "t".into(),
                    sender: "them".into(),
                    occurred_at: Some(1),
                    text: "hello".into(),
                    fingerprint: "fp1".into(),
                    service: None,
                },
                CensusInput {
                    source_id: 2,
                    thread_identifier: "t".into(),
                    sender: "them".into(),
                    occurred_at: Some(2),
                    text: "i will kill you".into(),
                    fingerprint: "fp2".into(),
                    service: None,
                },
            ],
            focus: 1,
        }
    }

    #[test]
    fn the_prompt_is_the_validated_structure() {
        let p = render_confirm_prompt(&window());
        // The hand-rolled Guard framing, not a chat template.
        assert!(p.starts_with("<|begin_of_text|><|start_header_id|>user<|end_header_id|>"));
        assert!(p.contains("<BEGIN UNSAFE CONTENT CATEGORIES>"));
        assert!(p.contains("S1: Threats and Violence."));
        assert!(p.contains("S9: Drugs and Illegal Activity."));
        // The judged message is repeated as the FINAL Agent turn, and the
        // assessment is scoped to it.
        let last_agent = p.rfind("Agent: i will kill you").unwrap();
        assert!(last_agent > p.find("<BEGIN CONVERSATION>").unwrap());
        assert!(p.contains("ONLY THE LAST Agent message"));
        assert!(p.ends_with("<|start_header_id|>assistant<|end_header_id|>\n\n"));
    }

    #[test]
    fn every_forensic9_category_is_present_exactly_once() {
        let p = render_confirm_prompt(&window());
        for i in 1..=9 {
            assert!(p.contains(&format!("S{i}: ")), "S{i} missing");
        }
        assert!(
            !p.contains("S10:"),
            "exactly nine categories — replace, not extend (§10.5)"
        );
    }

    #[test]
    fn replies_parse_as_keep_drop_or_neither() {
        assert_eq!(parse_confirm_reply("unsafe\nS1"), Some(true));
        assert_eq!(parse_confirm_reply("  Unsafe"), Some(true));
        assert_eq!(parse_confirm_reply("safe"), Some(false));
        assert_eq!(parse_confirm_reply("safe\n"), Some(false));
        // The §10.6 tell: the model echoing input instead of judging it. That
        // must surface as an error, not parse as either verdict.
        assert_eq!(parse_confirm_reply("...usual spot at 9safe"), None);
        assert_eq!(parse_confirm_reply(""), None);
    }
}
