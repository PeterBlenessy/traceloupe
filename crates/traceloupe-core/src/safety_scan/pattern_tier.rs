//! The coercive-control pattern tier (#529): detection from message
//! statistics, no ML, no text reading.
//!
//! Control lives in the pattern — volume, one-sidedness, contact resuming
//! after silence, night-time concentration — which is exactly what a
//! classifier reading one message cannot see and what sender+timestamp
//! columns already contain. The audit (PR #528) established that no public
//! dataset exists for this register; by design this tier needs none.
//!
//! Known blind spot, by upstream design: the census reads messages with
//! non-empty text, so an attachment-only barrage (100 photos overnight, no
//! caption) contributes nothing here. Recorded rather than hidden.
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
    /// Times the contact re-initiated with a BURST: after >= 12h of unanswered
    /// silence, sent 3+ messages within an hour. 12h, not 24, because a
    /// daily-noon rhythm has 23h54m gaps.
    ///
    /// The burst requirement is what separates control from information
    /// (#541). "Contacts you daily and you never reply" describes a stalker
    /// AND a nursery, a delivery driver, a recruiter and a landlord — all of
    /// which this tier flagged before the checklist was written. A person
    /// pressing for a response sends several messages at once; a broadcast
    /// sends one.
    pub reinitiations_unanswered: usize,
    /// Share of inbound sent between 00:00 and 06:00 UTC. UTC is a proxy —
    /// backups do not carry the sender's timezone; fixtures construct times
    /// accordingly and the rationale says "night-time" without a clock claim.
    pub night_share: f64,
}

const DAY: i64 = 86_400;
/// Messages within BURST_WINDOW that make a re-initiation a burst rather than
/// a notification. Fifteen minutes, not an hour: a nursery's four daily
/// updates span half an hour and were counted as a burst at the wider window
/// (#541). Someone pressing for a response types in minutes.
const BURST_MESSAGES: usize = 3;
const BURST_WINDOW: i64 = 900;
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
    let mut pending_reinit: Option<i64> = None;
    let mut burst_len = 0usize;
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
            if !owner_replied_since && m.at.saturating_sub(prev) >= DAY / 2 {
                // Provisional: confirmed below only if this opens a burst.
                pending_reinit = Some(m.at);
                burst_len = 1;
            } else if pending_reinit.is_some() && m.at.saturating_sub(prev) <= BURST_WINDOW {
                burst_len += 1;
                if burst_len == BURST_MESSAGES {
                    p.reinitiations_unanswered += 1;
                    pending_reinit = None;
                }
            } else if pending_reinit.is_some() {
                pending_reinit = None;
            }
        }
        last_inbound_at = Some(m.at);
        owner_replied_since = false;
    }
    if p.inbound > 0 {
        p.span_days = (last_in.saturating_sub(first_in) as f64 / DAY as f64).max(1.0 / 24.0);
        p.night_share = night as f64 / p.inbound as f64;
    }
    p
}

/// Automated senders — shortcodes ("262966"), alphanumeric sender IDs
/// ("AMZN", "DHL") — produce exactly the high-volume, never-answered,
/// re-initiating shape this tier hunts, and they are on every phone (2FA,
/// bank alerts, deliveries). A person's number is E.164: leading '+' and 10+
/// digits; an email is an iMessage handle. Anything else is treated as a
/// service, not a contact.
pub fn sender_is_service(handle: &str) -> bool {
    let h = handle.trim();
    if h == "me" || h.is_empty() {
        return false;
    }
    if h.contains('@') {
        return false; // email handle: a person
    }
    let digits = h.chars().filter(|c| c.is_ascii_digit()).count();
    if h.starts_with('+') && digits >= 10 {
        return false; // E.164: a person
    }
    if h.chars().any(|c| c.is_ascii_alphabetic()) {
        return true; // alphanumeric sender ID
    }
    digits < 7 // bare short numeric code
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
    // A sub-day thread has no PATTERN, whatever its volume — twenty unanswered
    // messages in twenty minutes is a friend venting while you're in a
    // meeting. Persistence means days.
    if p.span_days < 2.0 {
        return v;
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
    // night_share is COMPUTED but not a criterion: the 00-06 window is UTC,
    // which is a Tokyo working morning — "night-time" would be a false claim
    // in a forensic finding for most of the world. Re-enable only with the
    // owner's timezone (cache.health_timezones) resolving the window.
    let mut nonreciprocal = Vec::new();
    if reply_ratio <= 0.1 {
        nonreciprocal.push("rarely-answered");
    }
    // A long unanswered run is only evidence of non-reciprocity inside a
    // relationship that is ALREADY thin. A friend live-texting an event sends
    // twenty in a row and you answer them all day long — 27% reply rate, and
    // the tier flagged it before this qualifier (#541).
    if p.longest_one_sided_run >= 15 && reply_ratio <= 0.25 {
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
    let days = p.span_days.max(1.0).round() as i64;
    let mut parts = vec![format!(
        "{} messages over {} day{} with {} replies",
        p.inbound,
        days,
        if days == 1 { "" } else { "s" },
        p.outbound
    )];
    if v.criteria.contains(&"keeps-reinitiating-unanswered") {
        parts.push(format!(
            "resumed contact {} times after long unanswered gaps",
            p.reinitiations_unanswered
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
        assert!(
            v.flagged,
            "nightly check-ins still flag on re-initiation + non-reciprocity \
             (the night criterion itself is disabled until owner-timezone \
             resolution exists): {p:?}"
        );
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

    /// A daily unanswered drumbeat is a NOTIFICATION, not control — and this
    /// assertion used to say the opposite. The original comment called
    /// flagging it "the price of catching the stalking shape"; the #541
    /// checklist showed the price was nurseries, delivery drivers, recruiters
    /// and landlords, so the burst requirement now separates them and this
    /// test inverted.
    #[test]
    fn a_daily_unanswered_drumbeat_is_a_notification_not_control() {
        let mut msgs = Vec::new();
        for day in 0..30 {
            msgs.push((day * 86_400 + 12 * 3600, false));
        }
        let p = contact_pattern(&shape(&msgs));
        assert_eq!(
            p.reinitiations_unanswered, 0,
            "single messages are not bursts"
        );
        assert!(!classify(&p).flagged, "{p:?}");
    }

    /// #541: the shapes a real phone carries that are NOT control — measured
    /// against the shipped classifier, not a mirror of it. Each is built from
    /// its real timing, because the tier reads nothing else.
    #[test]
    fn legitimate_high_volume_threads_are_not_flagged() {
        // (note, inbound/day, days, replies, night-share, burst size)
        let cases: &[(&str, i64, i64, i64, bool, i64)] = &[
            (
                "nursery daily updates, parent rarely replies",
                2,
                30,
                3,
                false,
                2,
            ),
            ("delivery driver updates over months", 1, 60, 2, false, 1),
            (
                "chatty friend who monologues, you reply later",
                7,
                30,
                400,
                false,
                7,
            ),
            ("elderly parent sending clippings daily", 2, 45, 8, false, 2),
            (
                "night-shift partner texting through their shift",
                4,
                30,
                60,
                true,
                4,
            ),
            (
                "sports club broadcast from the coach's phone",
                1,
                40,
                1,
                false,
                1,
            ),
            ("recruiter chasing weekly, unanswered", 1, 21, 0, false, 1),
            (
                "landlord about works, mostly unanswered",
                1,
                25,
                3,
                false,
                1,
            ),
        ];
        let mut flagged = Vec::new();
        for &(note, per_day, days, replies, night, burst) in cases {
            let mut msgs = Vec::new();
            let hour = if night { 2 } else { 13 };
            for d in 0..days {
                for i in 0..per_day.max(1) {
                    for b in 0..burst.max(1) {
                        msgs.push((d * 86_400 + hour * 3600 + i * 1800 + b * 60, false));
                    }
                }
            }
            for r in 0..replies {
                msgs.push((
                    r * (days * 86_400 / replies.max(1)) + hour * 3600 + 900,
                    true,
                ));
            }
            let p = contact_pattern(&shape(&msgs));
            let v = classify(&p);
            if v.flagged {
                flagged.push((note, format!("{:?}", v.criteria), p.inbound, p.outbound));
            }
        }
        assert!(
            flagged.is_empty(),
            "the pattern tier flagged legitimate high-volume threads: {flagged:#?}"
        );
    }

    /// Review of #531, finding 2: automated senders match the stalking shape
    /// on every phone and must be recognised as services.
    #[test]
    fn service_senders_are_recognised() {
        for s in ["262966", "AMZN", "DHL-Info", "72404", "12345"] {
            assert!(sender_is_service(s), "{s} is a service");
        }
        for s in ["+15550009090", "+447700900123", "mum@example.com", "me"] {
            assert!(!sender_is_service(s), "{s} is a person");
        }
    }

    /// Review of #531, finding 5: a sub-day barrage has volume but no
    /// PATTERN — twenty unanswered messages in twenty minutes is a friend
    /// venting while the owner is in a meeting.
    #[test]
    fn a_sub_day_barrage_never_flags() {
        let msgs: Vec<(i64, bool)> = (0..25).map(|i| (i * 60, false)).collect();
        let p = contact_pattern(&shape(&msgs));
        assert!(!classify(&p).flagged, "{p:?}");
    }

    /// Review of #531, finding 4: every threshold pinned one step OUTSIDE its
    /// bar — before this, six simultaneous loosenings left the whole suite
    /// green.
    #[test]
    fn each_threshold_holds_one_step_outside_its_bar() {
        // 19 inbound (bar: 20) — daily unanswered, otherwise maximal shape.
        let msgs: Vec<(i64, bool)> = (0..19).map(|d| (d * 86_400, false)).collect();
        assert!(
            !classify(&contact_pattern(&shape(&msgs))).flagged,
            "inbound bar"
        );
        // 3 re-initiations (bar: 4) with no other persistence signal:
        // 20 inbound in 4 daily clusters over 3+ days, low per-day volume.
        let mut msgs = Vec::new();
        for day in 0..4i64 {
            for i in 0..5i64 {
                msgs.push((day * 86_400 + i * 60, false));
            }
        }
        let p = contact_pattern(&shape(&msgs));
        assert_eq!(p.reinitiations_unanswered, 3);
        assert!(p.inbound as f64 / p.span_days.max(1.0) < 15.0, "{p:?}");
        assert!(!classify(&p).flagged, "reinit bar: {p:?}");
        // reply ratio 3/20 = 0.15 (bar: 0.10) and runs under 15: reciprocity
        // defeats the flag even with re-initiations present.
        let mut msgs = Vec::new();
        for day in 0..10i64 {
            for i in 0..2i64 {
                msgs.push((day * 86_400 + i * 60, false));
            }
        }
        msgs.push((86_400 * 2 + 300, true));
        msgs.push((86_400 * 5 + 300, true));
        msgs.push((86_400 * 8 + 300, true));
        let p = contact_pattern(&shape(&msgs));
        let ratio = p.outbound as f64 / p.inbound as f64;
        assert!(ratio > 0.10 && ratio < 0.2, "{ratio}");
        assert!(p.longest_one_sided_run < 15, "{p:?}");
        assert!(!classify(&p).flagged, "reply-ratio bar: {p:?}");
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
