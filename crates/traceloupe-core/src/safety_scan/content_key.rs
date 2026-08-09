//! The identity a content-scoped rule matches on.
//!
//! "Mark all of her heart emoji safe" needs "the same heart emoji" to match
//! itself, and it does not, byte for byte: ❤️ carries a variation selector that
//! ❤ does not, 👍🏿 carries a skin tone, and ❤️❤️❤️ is the same gesture said
//! three times. Without normalization a rule would cover exactly one message
//! and nothing else.
//!
//! # This function also decides what may be generalized at all
//!
//! [`content_key`] returns `None` for anything too long to recur. That is not
//! an optimization — it is what keeps the offer honest. A rule keyed on a
//! 200-word message can never match a second message, so offering to make one
//! would be a lie, and a dialog that offers a rule covering nothing teaches
//! people to dismiss dialogs. Callers use `None` to mean "do not offer".
//!
//! # Known limitation: composed vs decomposed text
//!
//! No Unicode normalization form is applied, so "café" written as `e` + a
//! combining acute does not match the precomposed spelling. Fixing it means a
//! new dependency for a case that barely arises in short affirmations, and the
//! failure direction is the safe one: the rule does not match, so the finding
//! still surfaces. Revisit if real keys ever show it.

use super::chunker::MEDIA_MARKER;
use super::trivial;

/// Longest key worth building, in characters after normalization.
///
/// Short, closed-form content — reactions, "ok", "thanks", "haha" — is what
/// recurs verbatim often enough for a standing rule to mean anything. The exact
/// number is a judgement, not a discovery; it is deliberately small, because a
/// key that is too generous produces rules the user cannot predict.
const MAX_KEY_CHARS: usize = 24;

/// The identity for a content-scoped rule, or `None` when this content cannot
/// generalize and no rule should be offered for it.
///
/// Normalization: modifiers dropped (variation selectors, skin tones, ZWJ),
/// whitespace collapsed, casefolded, and runs of a repeated **non-alphanumeric**
/// character reduced to one — so ❤️❤️❤️ ≡ ❤️ and "!!!" ≡ "!", while "hello"
/// keeps both its l's. Collapsing letters too would quietly merge distinct
/// words, and a rule is not the place to be clever about that.
pub fn content_key(text: &str) -> Option<String> {
    // Something came with the words, and it will not be the same something next
    // time. An attachment message is never the "same message" twice.
    if text.contains(MEDIA_MARKER) {
        return None;
    }

    let mut out = String::new();
    let mut last: Option<char> = None;
    let mut pending_space = false;
    for c in text.chars() {
        if trivial::is_modifier(c) {
            // Whitespace separates; the rest is invisible and simply dropped.
            if c.is_whitespace() && !out.is_empty() {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            out.push(' ');
            last = Some(' ');
            pending_space = false;
        }
        for lower in c.to_lowercase() {
            // Repeats of emoji and punctuation are emphasis, not content.
            if Some(lower) == last && !lower.is_alphanumeric() {
                continue;
            }
            out.push(lower);
            last = Some(lower);
        }
    }

    let key = out.trim();
    if key.is_empty() || key.chars().count() > MAX_KEY_CHARS {
        return None;
    }
    Some(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case the feature exists for: every way of writing the same heart
    /// lands on one key, so a rule made from one covers the others.
    #[test]
    fn every_spelling_of_a_heart_shares_one_key() {
        let k = content_key("\u{2764}").unwrap();
        for text in [
            "\u{2764}\u{FE0F}",
            "\u{2764}\u{2764}\u{2764}",
            "\u{2764}\u{FE0F}\u{2764}\u{FE0F}\u{2764}\u{FE0F}",
            "  \u{2764}\u{FE0F}  ",
        ] {
            assert_eq!(content_key(text).as_deref(), Some(k.as_str()), "{text:?}");
        }
    }

    #[test]
    fn skin_tone_and_case_do_not_split_a_key() {
        assert_eq!(content_key("\u{1F44D}\u{1F3FF}"), content_key("\u{1F44D}"));
        assert_eq!(content_key("OK Thanks"), content_key("ok   thanks"));
    }

    /// Letters are never collapsed. "hello" and "helo" are different words, and
    /// a suppression rule is the wrong place to decide otherwise.
    #[test]
    fn repeated_letters_survive_but_repeated_emoji_do_not() {
        assert_eq!(content_key("hello").as_deref(), Some("hello"));
        assert_ne!(content_key("hello"), content_key("helo"));
        assert_eq!(content_key("!!!"), content_key("!"));
        assert_eq!(content_key("\u{1F602}\u{1F602}"), content_key("\u{1F602}"));
    }

    /// The gate on the widening offer. Long content cannot recur, so there is
    /// no honest rule to make from it.
    #[test]
    fn content_too_long_to_recur_has_no_key() {
        assert_eq!(
            content_key("i'll meet you at the station at half past six, don't be late"),
            None
        );
        assert!(content_key("thanks so much").is_some(), "short phrases do");
    }

    #[test]
    fn nothing_to_key_returns_none() {
        assert_eq!(content_key(""), None);
        assert_eq!(content_key("   "), None);
        assert_eq!(content_key("\u{FE0F}"), None, "modifiers alone are nothing");
    }

    /// An attachment is never the same twice, whatever the caption says.
    #[test]
    fn an_attachment_has_no_key() {
        assert_eq!(content_key(&format!("\u{2764}\n{MEDIA_MARKER}")), None);
    }

    /// The two predicates must agree about modifiers, or a rule could be
    /// offered for content the classifier already refuses to flag.
    #[test]
    fn shares_its_modifier_rules_with_the_contentless_check() {
        for text in ["\u{2764}\u{FE0F}", "\u{1F44D}\u{1F3FF}", "!!!"] {
            assert!(trivial::is_contentless(text), "{text:?}");
            assert!(content_key(text).is_some(), "{text:?} must be keyable");
        }
    }
}
