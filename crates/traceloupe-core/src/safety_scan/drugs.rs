//! The drugs tier: supply, not vocabulary.
//!
//! Every other harm in the taxonomy can be approached through what is said.
//! This one cannot. Drug slang has the worst collision profile of any category
//! here: Molly and Charlie are names, ice and snow are weather, weed and grass
//! are gardening, coke is a drink, pot is cookware, a gram is a unit, a script
//! is a prescription, and "you got any spare?" is about cigarettes, sugar or
//! phone chargers far more often than anything else. A word list alone would
//! accuse people of dealing for weeding the garden.
//!
//! So the rule is a CONJUNCTION, and that is the whole design: a substance
//! reference AND the shape of a supply arrangement — a quantity or price, or a
//! handover. Neither half fires alone. This is the lesson the scam tier paid
//! for (its classifier flagged 21 of 25 ordinary bank and delivery messages
//! because it had learned business register): structure generalises across
//! registers where vocabulary does not.
//!
//! What that buys and costs, measured against the legitimate-substance-talk
//! checklist: prescriptions, gardening, cooking quantities, recovery talk and
//! ordinary transactions all carry one half and stay silent. What it misses is
//! any deal conducted purely in code — and that is stated plainly rather than
//! papered over, because a coverage report that implies otherwise is worse than
//! no tier.
//!
//! No corpus, no download, no artefact: there is no permissively licensed
//! corpus of dealing conversations, and the biomedical "drug" datasets are
//! about pharmacology, not supply.

/// Points needed to call it supply. Reaching it requires BOTH halves — the
/// substance weights and the arrangement weights are sized so neither can get
/// there alone.
const FLAG_AT: u32 = 4;

struct Signal {
    weight: u32,
    label: &'static str,
    words: &'static [&'static str],
}

/// Substances specific enough that the word itself is rarely innocent. The
/// ambiguous ones — weed, grass, pot, coke, ice, snow, molly, charlie — are
/// DELIBERATELY ABSENT as standalone terms: they collide with gardening,
/// cooking, weather and human names, and the checklist proves it.
const SUBSTANCES: &[Signal] = &[
    Signal {
        weight: 2,
        label: "names a controlled drug",
        words: &[
            "mdma",
            "ketamine",
            "cocaine",
            "heroin",
            "meth",
            "crack",
            "fentanyl",
            "oxycontin",
            "oxycodone",
            "xanax",
            "diazepam",
            "valium",
            "adderall",
            "amphetamine",
            "lsd",
            "shrooms",
            "psilocybin",
            "cannabis",
            "hashish",
            "spice",
            "mephedrone",
            "ghb",
        ],
    },
    Signal {
        weight: 2,
        label: "uses supply slang for a drug",
        words: &[
            "zoot",
            "spliff",
            "doobie",
            "quarterbag",
            "ninebar",
            "wrap",
            "baggie",
            "bud",
            "gear",
            "pills",
            "tabs",
            "vials",
        ],
    },
];

/// The shape of an arrangement to supply. Each of these is ordinary on its own
/// — that is the point; they only count once a substance is present.
const ARRANGEMENT: &[Signal] = &[
    Signal {
        weight: 2,
        label: "quotes a quantity for sale",
        // "eighth" lives HERE, not with the substances: it is a quantity, and
        // an eighth of a tray of brownies is not a drug deal (the checklist
        // caught this the first time the tier was run).
        words: &[
            "gram", "grams", "oz", "ounce", "kilo", "half", "quarter", "eighth",
        ],
    },
    Signal {
        weight: 2,
        label: "arranges a handover",
        words: &[
            "drop", "dropping", "meet", "outside", "pickup", "collect", "post",
        ],
    },
    Signal {
        weight: 1,
        label: "asks about availability",
        words: &["got", "any", "sorted", "spare", "stock", "supply"],
    },
    Signal {
        weight: 1,
        label: "moves the conversation to another app",
        words: &["signal", "telegram", "wickr", "snap", "burner"],
    },
];

/// Whole-word matching, lowercased. A claim in a forensic report must be true
/// whenever it fires, and substring matching makes claims that are not:
/// "grammar" is not "gram", "gearbox" is not "gear".
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

/// A price: "20 quid", "£40", "$50", "40 each".
fn mentions_price(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains('£') || lower.contains('$') || lower.contains('€') {
        return true;
    }
    let toks = words(&lower);
    toks.windows(2).any(|w| {
        w[0].chars().all(|c| c.is_ascii_digit())
            && matches!(w[1].as_str(), "quid" | "each" | "notes" | "bag" | "bags")
    })
}

/// Supply score. Zero unless BOTH a substance and an arrangement are present —
/// the conjunction is enforced here, not left to the threshold.
pub fn score(text: &str) -> u32 {
    let toks = words(text);
    let hit = |sig: &Signal| sig.words.iter().any(|w| toks.iter().any(|t| t == w));
    let substance: u32 = SUBSTANCES.iter().filter(|s| hit(s)).map(|s| s.weight).sum();
    if substance == 0 {
        return 0;
    }
    let mut arrangement: u32 = ARRANGEMENT
        .iter()
        .filter(|s| hit(s))
        .map(|s| s.weight)
        .sum();
    if mentions_price(text) {
        arrangement += 2;
    }
    if arrangement == 0 {
        return 0;
    }
    substance + arrangement
}

/// The plain-language claims behind a score, for the finding's rationale. Only
/// signals that actually fired are listed — an explanation that names a signal
/// the text does not contain is a fabricated claim in a forensic report.
pub fn explain(text: &str) -> Vec<String> {
    let toks = words(text);
    let hit = |sig: &Signal| sig.words.iter().any(|w| toks.iter().any(|t| t == w));
    let mut out: Vec<String> = SUBSTANCES
        .iter()
        .chain(ARRANGEMENT.iter())
        .filter(|s| hit(s))
        .map(|s| s.label.to_string())
        .collect();
    if mentions_price(text) {
        out.push("quotes a price".to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conjunction is the design. Each half alone must score zero, however
    /// much of it there is.
    #[test]
    fn neither_half_fires_alone() {
        // Availability alone scores, but must not REACH the flag: "got any
        // mdma" is someone asking, not evidence of supply.
        assert!(
            score("got any mdma") < FLAG_AT,
            "availability alone must not flag"
        );
        assert_eq!(
            score("cannabis is legal in some states"),
            0,
            "substance alone"
        );
        assert_eq!(
            score("meet me outside at 8 with the cash, 20 quid"),
            0,
            "a whole arrangement with no substance is just a transaction"
        );
        assert!(
            score("2 grams of mdma, 40 quid, meet outside at 8") >= FLAG_AT,
            "both halves together is the case this tier exists for"
        );
    }

    /// The vocabulary this tier deliberately does NOT carry. Each of these
    /// words is a real drug term and a real ordinary word; the checklist is the
    /// measured version of this test.
    #[test]
    fn ambiguous_words_are_not_substances_on_their_own() {
        for text in [
            "spent all morning pulling weed out of the back beds, knackered",
            "the grass needs cutting before your mum gets here",
            "molly said she can do saturday, shall i book it for 4",
            "charlie owes me 20 quid from the taxi, remind him",
            "meet me outside greggs at 10 and ill give you the money for coke",
            "can you grab a bag of ice on the way, the freezer died",
            "an eighth of the tray is missing, someone got hungry",
        ] {
            assert_eq!(score(text), 0, "flagged ordinary talk: {text:?}");
        }
    }

    /// Prescriptions are the hardest legitimate case: real controlled drugs,
    /// real quantities, real prices.
    #[test]
    fn prescriptions_are_not_supply() {
        for text in [
            "picked up my prescription, 30 tablets, take one at night",
            "boots says the pills are ready, 9.90 for the script",
            "gp doubled my dose to 20mg, feeling rough today",
            "she is on morphine now, the hospice sorted it this morning",
            "dont take the codeine with the ibuprofen, it says on the box",
        ] {
            assert!(score(text) < FLAG_AT, "flagged a prescription: {text:?}");
        }
    }

    /// Recovery talk names drugs and must never be evidence of dealing.
    #[test]
    fn recovery_talk_is_never_supply() {
        for text in [
            "6 months clean today, thanks for sticking with me",
            "he relapsed last week, the family are gutted",
            "my sponsor says to call before i do anything stupid",
        ] {
            assert_eq!(score(text), 0, "flagged recovery: {text:?}");
        }
    }

    /// Whole words only: an explanation must not claim a signal the text does
    /// not contain.
    #[test]
    fn matching_is_on_whole_words() {
        assert_eq!(score("my grammar is terrible and the gearbox is going"), 0);
        assert!(explain("check the gearbox").is_empty());
    }

    /// THE gate, run on every build. Thirty ordinary messages that carry drug
    /// vocabulary, medical vocabulary or the shape of a transaction — gardening,
    /// prescriptions, recovery, names (Molly, Charlie), cooking quantities, and
    /// ordinary handovers. None is dealing and none may fire.
    ///
    /// The checklist is hand-written, so it is a BEHAVIOURAL gate rather than a
    /// headline number. Its measured counterpart: across 4,827 real personal
    /// SMS the tier fires once, on a message asking someone to source cannabis
    /// — a true positive sitting in the corpus's not-spam half.
    #[test]
    fn the_legitimate_substance_talk_checklist_stays_clean() {
        #[derive(serde::Deserialize)]
        struct Item {
            group: String,
            text: String,
        }
        #[derive(serde::Deserialize)]
        struct Checklist {
            messages: Vec<Item>,
        }
        let raw = include_str!("../../fixtures/safety-scan/legitimate-substance-talk.json");
        let list: Checklist = serde_json::from_str(raw).unwrap();
        assert!(list.messages.len() >= 30, "the gate must keep its coverage");
        let flagged: Vec<String> = list
            .messages
            .iter()
            .filter(|m| score(&m.text) >= FLAG_AT)
            .map(|m| format!("[{}] {}", m.group, m.text))
            .collect();
        assert!(flagged.is_empty(), "flagged ordinary traffic: {flagged:#?}");
    }

    /// Every claim in `explain` must correspond to something in the text.
    #[test]
    fn explanations_only_name_signals_that_fired() {
        let why = explain("2 grams of mdma, 40 quid, meet outside at 8");
        assert!(why.iter().any(|w| w.contains("controlled drug")));
        assert!(why.iter().any(|w| w.contains("quantity")));
        assert!(why.iter().any(|w| w.contains("price")));
        assert!(
            !why.iter().any(|w| w.contains("another app")),
            "named a signal the text does not contain"
        );
    }
}
