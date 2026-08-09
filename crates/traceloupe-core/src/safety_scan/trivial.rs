//! Contentless items: message bodies that cannot justify a Content Finding on
//! their own, no matter who sent them.
//!
//! A message whose whole content is "❤️" carries no standalone harm signal. It
//! is one of the commonest false alarms in real review, and unlike a genuine
//! misjudgement it is not a case the suppression rules should have to absorb —
//! a rule per sender per emoji is bookkeeping for something that was never a
//! finding.
//!
//! # Why an allowlist, and not "emoji only"
//!
//! The tempting rule is "any item that is nothing but emoji". It is wrong for a
//! safety tool: a lone 🔫 or 🔪 sent to someone is exactly the kind of wordless
//! message threat-violence exists to catch, and blanket-suppressing emoji-only
//! items would make it permanently invisible.
//!
//! So [`BENIGN`] is an allowlist of emoji that are affectionate or celebratory
//! and nothing else. An emoji that is not on it — because it is menacing,
//! because it is ambiguous, or merely because nobody has considered it yet —
//! leaves the item classifiable. The failure mode is a false alarm we already
//! have, never a silenced finding.
//!
//! Tapbacks need no handling here: the Messages parser folds them into the
//! target message's `reactions` column and never stores them as messages
//! (`parsers/messages.rs`), so they never reach the chunker at all.

use super::chunker::MEDIA_MARKER;

/// Emoji that mean warmth, agreement or celebration, and carry no threat in any
/// reading. Deliberately conservative: weapons, anger, drugs, sexual and
/// death-related emoji are all absent, as is anything whose tone depends on
/// context (👀, 🔥, 😈), because an item holding one of those should still reach
/// the classifier.
const BENIGN: &[char] = &[
    // Hearts
    '\u{2764}',  // ❤
    '\u{2665}',  // ♥
    '\u{1F9E1}', // 🧡
    '\u{1F49B}', // 💛
    '\u{1F49A}', // 💚
    '\u{1F499}', // 💙
    '\u{1F49C}', // 💜
    '\u{1F5A4}', // 🖤
    '\u{1F90D}', // 🤍
    '\u{1F90E}', // 🤎
    '\u{1F495}', // 💕
    '\u{1F496}', // 💖
    '\u{1F497}', // 💗
    '\u{1F498}', // 💘
    '\u{1F49D}', // 💝
    '\u{1F49E}', // 💞
    '\u{1F493}', // 💓
    '\u{1F49F}', // 💟
    '\u{1FAF6}', // 🫶
    // Gestures
    '\u{1F44D}', // 👍
    '\u{1F44C}', // 👌
    '\u{1F44F}', // 👏
    '\u{1F64C}', // 🙌
    '\u{1F64F}', // 🙏
    '\u{1F44B}', // 👋
    '\u{1F917}', // 🤗
    // Faces
    '\u{1F600}', // 😀
    '\u{1F603}', // 😃
    '\u{1F604}', // 😄
    '\u{1F601}', // 😁
    '\u{1F606}', // 😆
    '\u{1F605}', // 😅
    '\u{1F923}', // 🤣
    '\u{1F602}', // 😂
    '\u{1F642}', // 🙂
    '\u{1F60A}', // 😊
    '\u{1F607}', // 😇
    '\u{1F609}', // 😉
    '\u{1F60D}', // 😍
    '\u{1F970}', // 🥰
    '\u{1F618}', // 😘
    '\u{1F617}', // 😗
    '\u{1F619}', // 😙
    '\u{1F61A}', // 😚
    '\u{263A}',  // ☺
    '\u{1F929}', // 🤩
    '\u{1F973}', // 🥳
    // Celebration
    '\u{2728}',  // ✨
    '\u{1F389}', // 🎉
    '\u{1F38A}', // 🎊
    '\u{1F338}', // 🌸
    '\u{1F339}', // 🌹
    '\u{1F33A}', // 🌺
    '\u{1F490}', // 💐
    '\u{2705}',  // ✅
];

/// Characters that carry no meaning of their own and are skipped before the
/// item is judged: whitespace, emoji presentation selectors, skin-tone
/// modifiers, and the zero-width joiner that builds compound emoji.
fn is_modifier(c: char) -> bool {
    c.is_whitespace()
        || matches!(c, '\u{FE0F}' | '\u{FE0E}' | '\u{200D}')
        || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// True when `text` cannot support a finding on its own.
///
/// An item qualifies only when every significant character is either ASCII
/// punctuation or an emoji from [`BENIGN`]. One letter, one digit, one
/// unrecognised symbol, or an attachment marker is enough to disqualify it —
/// the bar is deliberately hard to clear.
pub fn is_contentless(text: &str) -> bool {
    // An attachment the model was told about but could not examine is not a
    // contentless item; whatever the words are, something came with them.
    if text.contains(MEDIA_MARKER) {
        return false;
    }
    for c in text.chars() {
        if is_modifier(c) {
            continue;
        }
        // A letter or a digit in any script is content: "911", "ok", "нет".
        if c.is_alphanumeric() {
            return false;
        }
        if c.is_ascii_punctuation() || BENIGN.contains(&c) {
            continue;
        }
        // Anything else — an unlisted emoji, a non-ASCII symbol — stays
        // classifiable. Unknown means flaggable, never silenced.
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_heart_is_contentless() {
        for text in [
            "\u{2764}",
            "\u{2764}\u{FE0F}",
            "\u{2764}\u{FE0F}\u{2764}\u{FE0F}\u{2764}\u{FE0F}",
            "  \u{1F44D}  ",
            "\u{1F44D}\u{1F3FF}", // skin tone applied
            "\u{1F602}\u{1F602}",
            "!!!",
            "?!",
        ] {
            assert!(is_contentless(text), "expected contentless: {text:?}");
        }
    }

    /// The reason this is an allowlist. A wordless threat must stay
    /// classifiable, and so must anything nobody has categorised yet.
    #[test]
    fn a_menacing_or_unlisted_emoji_stays_classifiable() {
        for text in [
            "\u{1F52B}",         // 🔫
            "\u{1F52A}",         // 🔪
            "\u{1F480}",         // 💀
            "\u{1F621}",         // 😡
            "\u{1F595}",         // 🖕
            "\u{1F346}",         // 🍆
            "\u{1F4A9}",         // 💩 — merely unlisted, not menacing
            "\u{2764}\u{1F52B}", // one benign, one not
        ] {
            assert!(!is_contentless(text), "expected classifiable: {text:?}");
        }
    }

    #[test]
    fn words_and_numbers_are_content() {
        for text in [
            "ok",
            "911",
            "\u{043D}\u{0435}\u{0442}", // нет
            "\u{2764} love you",
            "I'm fine \u{1F44D}",
            "\u{4F60}\u{597D}", // 你好
        ] {
            assert!(!is_contentless(text), "expected content: {text:?}");
        }
    }

    /// An attachment came with the words, and the model was told so. Whatever
    /// the caption is, the item is not empty.
    #[test]
    fn an_attachment_marker_disqualifies() {
        assert!(!is_contentless(&format!("\u{2764}\n{MEDIA_MARKER}")));
    }
}
