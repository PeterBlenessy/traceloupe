//! The scam/smishing tier (#539): structural rules, no model, no artefact.
//!
//! This design was chosen, then abandoned for a classifier, then chosen again
//! — and the reversal is the interesting part. Against the public SMS corpora
//! a TF-IDF classifier caught 92% where these rules caught 46%, so the
//! classifier shipped. A review then measured both against the register those
//! corpora lack: legitimate transactional SMS (banks, couriers, appointment
//! reminders, 2FA). UCI's "ham" is 0.17% URLs and almost entirely personal
//! chat, so the classifier had learned *business register ⇒ scam*:
//!
//! | at equal legitimate-traffic cost | real smishing caught | legitimate flagged |
//! |---|---|---|
//! | TF-IDF classifier @0.92 | 41% | 0/25 |
//! | TF-IDF classifier @0.47 (its tuned point) | 92% | **21/25** |
//! | **these rules** | **46%** | **0/25** |
//!
//! At its tuned threshold the classifier flagged 21 of 25 ordinary bank and
//! delivery messages; forced down to where it spares them, it catches less
//! than the rules. Structure generalises across registers where vocabulary
//! does not — so the rules ship, and a 540 KB weights blob, its parity test,
//! its attribution burden and its tokenisation-drift risk all disappear with
//! the classifier.
//!
//! Cost: nothing to download, nothing to parse, nanoseconds per message.

/// One structural signal, its weight, and the plain-language claim it licenses.
/// The claim is a statement inside a forensic report, so it must be true
/// whenever the signal fires — matching is on whole words for that reason.
struct Signal {
    weight: u32,
    label: &'static str,
    words: &'static [&'static str],
}

/// Weights reflect measured lift over ordinary SMS: premium-rate numbers 295x,
/// links 71x, money claims 43x, urgency 34x, the rest single-digit.
const SIGNALS: &[Signal] = &[
    Signal {
        weight: 2,
        label: "claims you have won something",
        words: &["won", "winner", "prize", "award", "guaranteed"],
    },
    Signal {
        weight: 2,
        label: "presses for immediate action",
        words: &["urgent", "immediately", "expires", "expired"],
    },
    Signal {
        weight: 1,
        label: "offers something free",
        words: &["free"],
    },
    Signal {
        weight: 1,
        label: "asks you to verify account details",
        words: &["verify"],
    },
    Signal {
        weight: 1,
        label: "asks for credentials",
        words: &["password", "passcode"],
    },
    Signal {
        weight: 1,
        label: "references a delivery",
        words: &["parcel", "shipment", "customs", "courier"],
    },
    Signal {
        weight: 1,
        label: "asks you to claim something",
        words: &["claim", "claiming", "collect"],
    },
    // Added after measuring each against BOTH populations: fires on this
    // share of real smishing / of ordinary SMS / of the legitimate
    // transactional checklist. "call now"-style wording was rejected despite
    // 41% scam coverage — it also fires on 5.7% of ordinary SMS and on a real
    // bank message ("Call the number on your card").
    Signal {
        weight: 1,
        // 20% scam, 0.2% ham, 0/25 legitimate
        label: "offers money or credit",
        words: &["cash", "cashback", "bonus"],
    },
    Signal {
        weight: 1,
        // 24% scam, 1.7% ham, 0/25 legitimate
        label: "asks you to reply with a code",
        words: &["txt"],
    },
    Signal {
        weight: 2,
        // 12% scam, 0.1% ham, 0/25 legitimate — prize-draw framing is close
        // to definitional for this category
        label: "claims you were selected or have won a draw",
        words: &["draw", "selected", "chosen", "congratulations"],
    },
    Signal {
        weight: 1,
        // 5% scam, 0.0% ham, 0/25 legitimate
        label: "uses premium-subscription wording",
        words: &["unsubscribe", "subscription", "poly", "tone"],
    },
];

/// Combined score must reach this for a message to be reported. Chosen on the
/// public corpora and verified against the legitimate-transactional checklist
/// (0 of 25 flagged) — a single signal is never enough, because ordinary
/// service messages legitimately contain links, deadlines and the word "free".
const FLAG_AT: u32 = 4;

/// Apostrophes stay INSIDE words: splitting on them turns "won't" into "won"
/// and resurrects the fabricated-claim bug from a different direction. With
/// the classifier gone there is no tokenisation to stay parity with, so this
/// can simply be correct.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\'' || c == '\u{2019}'))
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_string)
        .collect()
}

/// UK premium-rate ranges ONLY: 09xx, 087x, 084x. 0800/0808 are freephone;
/// calling them premium-rate was a false statement that fired on 227 of 379
/// matches (#540 review). The digit run must stand alone, so order numbers
/// and account numbers no longer trip it.
fn premium_rate_number(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|run| (10..=11).contains(&run.len()))
        .any(|r| r.starts_with("09") || r.starts_with("087") || r.starts_with("084"))
}

fn has_link(lower: &str) -> bool {
    lower.contains("http") || lower.contains("www.") || lower.contains("bit.ly")
}

/// The signals a message actually exhibits — the readable reason a finding
/// exists, and the input to the score.
pub fn explain(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    let ws = words(text);
    let mut out = Vec::new();
    if has_link(&lower) {
        out.push("contains a link");
    }
    if premium_rate_number(&lower) {
        out.push("asks you to call a premium-rate number");
    }
    for s in SIGNALS {
        if s.words.iter().any(|w| ws.iter().any(|t| t == w)) && !out.contains(&s.label) {
            out.push(s.label);
        }
    }
    out
}

pub fn score(text: &str) -> u32 {
    let lower = text.to_lowercase();
    let ws = words(text);
    let mut total = 0;
    if has_link(&lower) {
        total += 3;
    }
    if premium_rate_number(&lower) {
        total += 3;
    }
    for s in SIGNALS {
        if s.words.iter().any(|w| ws.iter().any(|t| t == w)) {
            total += s.weight;
        }
    }
    total
}

pub fn is_scam(text: &str) -> bool {
    score(text) >= FLAG_AT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_messages_do_not_flag() {
        for t in [
            "can you grab milk on the way home",
            "the meeting moved to 3pm, see you there",
            "happy birthday!! hope you have a lovely day x",
            "running about 20 minutes late, sorry",
        ] {
            assert!(!is_scam(t), "false alarm on {t:?} (score {})", score(t));
        }
    }

    /// The register the classifier could not survive: real service messages
    /// legitimately carry links, deadlines, money and the word "free".
    #[test]
    fn legitimate_transactional_messages_do_not_flag() {
        for t in [
            "HSBC: You have authorised a payment of GBP 45.00 to AMAZON UK on 18/08. Not you? Call the number on your card.",
            "Your Uber code is 4821. Enter it to sign in.",
            "Your Royal Mail parcel is out for delivery today between 09:00 and 13:00.",
            "DPD: Your parcel is running late and will arrive tomorrow. Track at dpd.co.uk/track",
            "TV Licence: your licence is due for renewal on 01/09. Renew at tvlicensing.co.uk",
            "Your appointment with Dr Patel is confirmed for 22 Aug at 11:00. Reply CANCEL to change.",
            "Santander: We've spotted unusual activity on your card ending 4417. Reply YES if this was you.",
            "Your prescription is ready for collection at Boots High Street.",
        ] {
            assert!(
                !is_scam(t),
                "legitimate message flagged (score {}): {t:?}",
                score(t)
            );
        }
    }

    #[test]
    fn real_shaped_smishing_flags() {
        for t in [
            "URGENT: your parcel is held at customs. Pay the 1.45 GBP fee now at http://rm-delivery-fee.co/uk or it returns to sender",
            "You have WON a guaranteed GBP 1000 cash prize! Call 09061234567 now to claim",
            "FREE entry to win a prize draw! Text WIN to claim your award now",
        ] {
            assert!(is_scam(t), "missed smishing (score {}): {t:?}", score(t));
        }
    }

    /// Claims must be true whenever they appear: substring matching once put
    /// "claims you have won something" on "wondering" and "won't", and called
    /// freephone numbers premium-rate (#540 review, finding 2).
    #[test]
    fn explanations_never_fabricate() {
        for t in [
            "just wondering when you're free tomorrow",
            "that was a wonderful evening, thank you",
            "he won't be back till 9",
        ] {
            assert!(
                !explain(t).contains(&"claims you have won something"),
                "fabricated a win claim for {t:?}: {:?}",
                explain(t)
            );
        }
        for t in [
            "Call MobilesDirect free on 08000938767 to update now",
            "my order number is 0812345678901",
        ] {
            assert!(
                !explain(t).contains(&"asks you to call a premium-rate number"),
                "called a non-premium number premium in {t:?}"
            );
        }
        assert!(explain("Call 09061234567 to claim")
            .contains(&"asks you to call a premium-rate number"));
        assert!(explain("see you at 8").is_empty());
    }
}
