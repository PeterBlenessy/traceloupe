//! The lexical scorer shared by the tiers that ship word weights instead of a
//! network: the deep-scan router (#544) and the hate tier (#549).
//!
//! It reproduces scikit-learn's `TfidfVectorizer(ngram_range=(1,2),
//! sublinear_tf=True, strip_accents="unicode") + LogisticRegression` exactly.
//! That kind of reimplementation fails silently — a different accent rule, a
//! bigram joined with the wrong separator, tf without the log — so each model
//! that uses it ships a parity fixture of texts with scikit-learn's own
//! probabilities, and a test that must agree.
//!
//! Why word weights keep winning here: measured twice against a ModernBERT
//! trained on the same data, the network matched it at routing (0.991/0.792,
//! identical) and lost at the precision a findings tier needs (28% vs 20% of
//! message-length hate caught, both with zero false alarms on 4,827 real
//! personal messages). It costs 150 MB of download and milliseconds per
//! message; this costs kilobytes and microseconds.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Deserialize)]
struct RawTerm {
    t: String,
    idf: f32,
    w: f32,
}

#[derive(Deserialize)]
struct RawModel {
    intercept: f32,
    #[serde(default)]
    threshold: Option<f32>,
    terms: Vec<RawTerm>,
}

pub struct LexicalModel {
    /// term -> (idf, weight). 1-grams and 2-grams; a 2-gram is its two tokens
    /// joined by one space, as scikit-learn writes them.
    terms: HashMap<String, (f32, f32)>,
    intercept: f32,
    /// Present only for tiers that DECIDE. The router has none by design: it
    /// ranks, and a rank cannot drift into an unexamined decision boundary.
    threshold: Option<f32>,
}

impl LexicalModel {
    pub fn from_json(raw: &str) -> Self {
        let m: RawModel = serde_json::from_str(raw)
            .expect("lexical models ship with the binary; a parse failure is a build error");
        Self {
            terms: m.terms.into_iter().map(|t| (t.t, (t.idf, t.w))).collect(),
            intercept: m.intercept,
            threshold: m.threshold,
        }
    }

    /// The calibrated cut, for tiers that have one.
    pub fn threshold(&self) -> Option<f32> {
        self.threshold
    }

    /// scikit-learn's preprocessing, in order: lowercase, THEN strip accents
    /// (NFKD, drop combining marks). Reversing the order changes the tokens for
    /// some scripts; the parity fixture covers it.
    fn normalise(text: &str) -> String {
        use unicode_normalization::UnicodeNormalization;
        text.to_lowercase()
            .nfkd()
            .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
            .collect()
    }

    /// `(?u)\b\w\w+\b`: maximal runs of word characters, keeping those of two
    /// or more. After NFKD stripping there are no combining marks left.
    fn tokenize(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() || c == '_' {
                cur.push(c);
            } else if cur.chars().count() >= 2 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
        if cur.chars().count() >= 2 {
            out.push(cur);
        }
        out
    }

    /// One text's score in [0, 1]: sublinear tf (`1 + ln(count)`) times the
    /// trained idf, l2-normalised over the terms PRESENT, then the logistic
    /// link. Terms outside the vocabulary are dropped before the norm, which is
    /// what scikit-learn does and is load-bearing for parity.
    pub fn score(&self, text: &str) -> f32 {
        let toks = Self::tokenize(&Self::normalise(text));
        let mut bigrams: Vec<String> = Vec::with_capacity(toks.len().saturating_sub(1));
        for pair in toks.windows(2) {
            bigrams.push(format!("{} {}", pair[0], pair[1]));
        }
        let mut counts: HashMap<&str, f32> = HashMap::new();
        for gram in toks
            .iter()
            .map(String::as_str)
            .chain(bigrams.iter().map(String::as_str))
        {
            if let Some((term, _)) = self.terms.get_key_value(gram) {
                *counts.entry(term.as_str()).or_insert(0.0) += 1.0;
            }
        }
        if counts.is_empty() {
            return sigmoid(self.intercept);
        }
        let mut vec: Vec<(f32, f32)> = Vec::with_capacity(counts.len());
        for (term, count) in &counts {
            let (idf, w) = self.terms[*term];
            vec.push(((1.0 + count.ln()) * idf, w));
        }
        let norm = vec.iter().map(|(v, _)| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return sigmoid(self.intercept);
        }
        let z = vec.iter().map(|(v, w)| (v / norm) * w).sum::<f32>() + self.intercept;
        sigmoid(z)
    }
}

fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_follow_scikit_learns_pattern() {
        let t = |s: &str| LexicalModel::tokenize(&LexicalModel::normalise(s));
        assert_eq!(t("hi a bc"), vec!["hi", "bc"], "two-character minimum");
        assert_eq!(t("Café RENOVÉ"), vec!["cafe", "renove"], "accents folded");
        assert_eq!(
            t("a_b c-d 12"),
            vec!["a_b", "12"],
            "underscore is a word char"
        );
        assert!(t("¿ x ?").is_empty());
    }
}
