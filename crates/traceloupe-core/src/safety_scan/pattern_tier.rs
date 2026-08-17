//! The coercive-control pattern tier (#529): detection from message
//! statistics, no ML, no text reading.
//!
//! Control lives in the pattern — volume, one-sidedness, contact resuming
//! after silence, night-time concentration — which is exactly what a
//! classifier reading one message cannot see and what sender+timestamp
//! columns already contain. The audit (PR #528) established that no public
//! dataset exists for this register; by design this tier needs none.
//!
//! Everything here is pure and unit-tested against hand-designed shapes: the
//! stalking shapes the tier must flag, and the heavy-but-ordinary shapes
//! (group planning bursts, chatty friends) it must not.

/// One message's metadata — all the tier ever sees.
#[derive(Debug, Clone, Copy)]
pub struct MsgMeta {
    /// Unix seconds.
    pub at: i64,
    /// True when the phone's owner sent it.
    pub from_me: bool,
}

/// Per-thread contact-pattern statistics for the non-owner side.
#[derive(Debug, Clone, Default)]
pub struct ContactPattern {
    /// Messages from the contact.
    pub inbound: usize,
    /// Messages from the owner.
    pub outbound: usize,
    /// Span between first and last inbound, in days (>= 1/24 once any exist).
    pub span_days: f64,
    /// Longest run of consecutive inbound messages with no reply between.
    pub longest_one_sided_run: usize,
    /// Times the contact re-initiated: sent again after >= 12h of silence
    /// during which the owner never replied. 12h, not 24: a daily-noon burst
    /// rhythm has 23h54m gaps and "a day" would miss it by six minutes — the
    /// signal is overnight-scale persistence, not a calendar day.
    pub reinitiations_unanswered: usize,
    /// Share of inbound sent between 00:00 and 06:00 UTC. UTC is a proxy —
    /// backups do not carry the sender's timezone; fixtures construct times
    /// accordingly and the rationale says "night-time" without a clock claim.
    pub night_share: f64,
}

const DAY: i64 = 86_400;
const NIGHT_START_H: i64 = 0;
const NIGHT_END_H: i64 = 6;

pub fn contact_pattern(msgs: &[MsgMeta]) -> ContactPattern {
    let mut p = ContactPattern::default();
    let mut sorted: Vec<MsgMeta> = msgs.to_vec();
    sorted.sort_by_key(|m| m.at);
    let (mut first_in, mut last_in) = (i64::MAX, i64::MIN);
    let mut run = 0usize;
    let mut night = 0usize;
    // Last inbound timestamp, and whether the owner has replied since it —
    // the pair that defines an unanswered re-initiation.
    let mut last_inbound_at: Option<i64> = None;
    let mut owner_replied_since = true;
    for m in &sorted {
        if m.from_me {
            p.outbound += 1;
            run = 0;
            owner_replied_since = true;
            continue;
        }
        p.inbound += 1;
        first_in = first_in.min(m.at);
        last_in = last_in.max(m.at);
        run += 1;
        p.longest_one_sided_run = p.longest_one_sided_run.max(run);
        let hour = m.at.rem_euclid(DAY) / 3600;
        if (NIGHT_START_H..NIGHT_END_H).contains(&hour) {
            night += 1;
        }
        if let Some(prev) = last_inbound_at {
            if !owner_replied_since && m.at - prev >= DAY / 2 {
                p.reinitiations_unanswered += 1;
            }
        }
        last_inbound_at = Some(m.at);
        owner_replied_since = false;
    }
    if p.inbound > 0 {
        p.span_days = ((last_in - first_in) as f64 / DAY as f64).max(1.0 / 24.0);
        p.night_share = night as f64 / p.inbound as f64;
    }
    p
}

/// The verdict, with which criteria fired — the rationale is built from these
/// so it can never claim something the numbers don't show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PatternVerdict {
    pub flagged: bool,
    pub criteria: Vec<&'static str>,
}

/// Conservative thresholds, pinned by the shape tests below and to be tuned
/// on fixture backups before shipping (#529 acceptance). Flag needs BOTH a
/// volume/persistence signal AND a non-reciprocity signal — heavy alone is a
/// chatty friend, one-sided alone is a newsletter.
pub fn classify(p: &ContactPattern) -> PatternVerdict {
    let mut v = PatternVerdict::default();
    if p.inbound < 20 {
        return v; // below any meaningful pattern
    }
    let reply_ratio = p.outbound as f64 / p.inbound as f64;
    let per_day = p.inbound as f64 / p.span_days.max(1.0);

    let mut persistence = Vec::new();
    if per_day >= 15.0 {
        persistence.push("high-volume");
    }
    if p.reinitiations_unanswered >= 4 {
        persistence.push("keeps-reinitiating-unanswered");
    }
    if p.night_share >= 0.4 {
        persistence.push("night-concentrated");
    }
    let mut nonreciprocal = Vec::new();
    if reply_ratio <= 0.1 {
        nonreciprocal.push("rarely-answered");
    }
    if p.longest_one_sided_run >= 15 {
        nonreciprocal.push("long-one-sided-runs");
    }
    if !persistence.is_empty() && !nonreciprocal.is_empty() {
        v.flagged = true;
        v.criteria = persistence.into_iter().chain(nonreciprocal).collect();
    }
    v
}

/// The plain-language rationale, from the numbers only.
pub fn rationale(p: &ContactPattern, v: &PatternVerdict) -> String {
    let mut parts = vec![format!(
        "{} messages over {:.0} days with {} replies",
        p.inbound,
        p.span_days.max(1.0),
        p.outbound
    )];
    if v.criteria.contains(&"keeps-reinitiating-unanswered") {
        parts.push(format!(
            "resumed contact {} times after long unanswered gaps",
            p.reinitiations_unanswered
        ));
    }
    if v.criteria.contains(&"night-concentrated") {
        parts.push(format!(
            "{:.0}% sent in night-time hours",
            p.night_share * 100.0
        ));
    }
    if v.criteria.contains(&"long-one-sided-runs") {
        parts.push(format!(
            "up to {} messages in a row without a reply",
            p.longest_one_sided_run
        ));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(items: &[(i64, bool)]) -> Vec<MsgMeta> {
        items
            .iter()
            .map(|&(at, from_me)| MsgMeta { at, from_me })
            .collect()
    }

    /// The 47-messages-after-block shape: daily unanswered bursts for a week.
    #[test]
    fn a_blocked_ex_sending_daily_unanswered_bursts_is_flagged() {
        let mut msgs = Vec::new();
        for day in 0..7 {
            for i in 0..7 {
                // Midday bursts, no reply ever.
                msgs.push((day * 86_400 + 12 * 3600 + i * 60, false));
            }
        }
        let p = contact_pattern(&shape(&msgs));
        assert_eq!(p.inbound, 49);
        assert_eq!(p.outbound, 0);
        assert!(
            p.reinitiations_unanswered >= 5,
            "daily bursts have overnight gaps: {p:?}"
        );
        let v = classify(&p);
        assert!(v.flagged, "{p:?}");
        let r = rationale(&p, &v);
        assert!(r.contains("49 messages"), "{r}");
        assert!(r.contains("resumed contact"), "{r}");
    }

    /// Nightly check-ins: moderate volume, night-concentrated, barely answered.
    #[test]
    fn nightly_checkins_barely_answered_are_flagged() {
        let mut msgs = Vec::new();
        for day in 0..14 {
            for i in 0..3 {
                msgs.push((day * 86_400 + 2 * 3600 + i * 300, false)); // 02:00
            }
        }
        msgs.push((5 * 86_400 + 2 * 3600 + 100, true)); // one reply in two weeks
        let p = contact_pattern(&shape(&msgs));
        let v = classify(&p);
        assert!(v.flagged, "{p:?}");
        assert!(v.criteria.contains(&"night-concentrated"), "{v:?}");
    }

    /// A group-planning burst: heavy, but the owner replies constantly.
    #[test]
    fn a_heavy_planning_burst_with_replies_is_not_flagged() {
        let mut msgs = Vec::new();
        for i in 0..120 {
            msgs.push((i * 600, i % 3 == 0)); // owner sends every third message
        }
        let p = contact_pattern(&shape(&msgs));
        let v = classify(&p);
        assert!(!v.flagged, "reciprocal traffic must never flag: {p:?}");
    }

    /// A newsletter/notification shape: fully one-sided but never re-initiating
    /// in the unanswered sense day after day at low volume... it IS
    /// re-initiating. What saves it must be volume + the persistence bar:
    /// one message a day is not a burst, not night-time, and re-initiations
    /// alone without any other persistence signal still flag — so this test
    /// pins the deliberate trade: a daily unanswered drumbeat over weeks IS
    /// flagged. Marketing spam that matches a stalking shape is the price of
    /// catching the stalking shape; the deep scan's text tier tells them
    /// apart downstream.
    #[test]
    fn a_daily_unanswered_drumbeat_flags_by_design() {
        let mut msgs = Vec::new();
        for day in 0..30 {
            msgs.push((day * 86_400 + 12 * 3600, false));
        }
        let p = contact_pattern(&shape(&msgs));
        let v = classify(&p);
        assert!(v.flagged, "{p:?}");
    }

    /// Below the volume floor nothing flags, whatever the shape.
    #[test]
    fn tiny_threads_never_flag() {
        let msgs: Vec<(i64, bool)> = (0..10).map(|d| (d * 86_400, false)).collect();
        let p = contact_pattern(&shape(&msgs));
        assert!(!classify(&p).flagged);
    }

    /// The rationale never claims a criterion that did not fire.
    #[test]
    fn the_rationale_matches_the_criteria() {
        let mut msgs = Vec::new();
        for day in 0..7 {
            for i in 0..8 {
                msgs.push((day * 86_400 + 12 * 3600 + i * 60, false));
            }
        }
        let p = contact_pattern(&shape(&msgs));
        let v = classify(&p);
        let r = rationale(&p, &v);
        if !v.criteria.contains(&"night-concentrated") {
            assert!(!r.contains("night-time"), "{r}");
        }
    }
}
