//! Feed-loader tests against frozen copies of the real indicator feeds
//! (fixtures/indicators/, downloaded 2026-07-20) plus a synthetic bundle for
//! the failure paths. Counts are exact for the frozen files.

use traceloupe_core::indicators::{
    load_echap_yaml, load_stix_bundle, FeedClass, IndicatorKind, IndicatorSet, Severity,
};

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/indicators/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("fixture readable")
}

#[test]
fn pegasus_stix_loads_fully() {
    let feed = load_stix_bundle(&fixture("pegasus.stix2"), "amnesty/pegasus", FeedClass::Mercenary)
        .expect("parses");
    assert_eq!(feed.indicators.len(), 1549);
    assert!(feed.skipped.is_empty(), "skipped: {:?}", &feed.skipped[..5.min(feed.skipped.len())]);

    let by = |k| feed.indicators.iter().filter(|i| i.kind == k).count();
    assert_eq!(by(IndicatorKind::Domain), 1438);
    assert_eq!(by(IndicatorKind::ProcessName), 81);
    assert_eq!(by(IndicatorKind::FileName), 1);
    assert_eq!(by(IndicatorKind::Email), 29);

    // Attribution flows through the relationship objects.
    assert!(feed.indicators.iter().all(|i| i.malware == "Pegasus"));
    // Severity by kind for mercenary feeds.
    assert!(feed
        .indicators
        .iter()
        .all(|i| match i.kind {
            IndicatorKind::Domain | IndicatorKind::Email => i.severity == Severity::Warning,
            IndicatorKind::ProcessName | IndicatorKind::FileName =>
                i.severity == Severity::Critical,
            _ => true,
        }));
    // Values are normalized.
    assert!(feed
        .indicators
        .iter()
        .filter(|i| i.kind == IndicatorKind::Domain)
        .all(|i| i.value == i.value.to_ascii_lowercase()));
}

#[test]
fn kingspawn_stix_loads_fully() {
    let feed = load_stix_bundle(
        &fixture("kingspawn.stix2"),
        "imazing/kingspawn",
        FeedClass::Mercenary,
    )
    .expect("parses");
    assert_eq!(feed.indicators.len(), 206);
    assert!(feed.skipped.is_empty());
    assert_eq!(
        feed.indicators
            .iter()
            .filter(|i| i.kind == IndicatorKind::Domain)
            .count(),
        204
    );
    assert_eq!(
        feed.indicators
            .iter()
            .filter(|i| i.kind == IndicatorKind::ProcessName)
            .count(),
        2
    );
}

#[test]
fn echap_ioc_yaml_loads_fully() {
    let feed = load_echap_yaml(&fixture("ioc.yaml"), "echap/ioc", FeedClass::Stalkerware)
        .expect("parses");
    assert_eq!(feed.indicators.len(), 2746);

    let by = |k| feed.indicators.iter().filter(|i| i.kind == k).count();
    assert_eq!(by(IndicatorKind::BundleId), 616);
    assert_eq!(by(IndicatorKind::CertSha1), 472);
    assert_eq!(by(IndicatorKind::Domain), 1592);
    assert_eq!(by(IndicatorKind::Ip), 66);

    // Installed stalkerware packages are the strongest signal.
    assert!(feed
        .indicators
        .iter()
        .filter(|i| i.kind == IndicatorKind::BundleId)
        .all(|i| i.severity == Severity::Critical));
    // Spot-check one family end to end.
    assert!(feed
        .indicators
        .iter()
        .any(|i| i.kind == IndicatorKind::BundleId
            && i.value == "com.fone"
            && i.malware == "TheTruthSpy"));
    // Vendor website (Info) vs C2 endpoint (Warning) for the same family:
    let copy9: Vec<_> = feed
        .indicators
        .iter()
        .filter(|i| i.value == "copy9.com" && i.malware == "TheTruthSpy")
        .collect();
    assert!(copy9.iter().any(|i| i.severity == Severity::Info));
    assert!(copy9.iter().any(|i| i.severity == Severity::Warning));
}

#[test]
fn echap_watchware_is_all_info() {
    let feed = load_echap_yaml(&fixture("watchware.yaml"), "echap/watchware", FeedClass::Watchware)
        .expect("parses");
    assert_eq!(feed.indicators.len(), 159);
    assert!(feed.indicators.iter().all(|i| i.severity == Severity::Info));
}

#[test]
fn merged_set_dedupes_within_and_across_feeds() {
    let ioc = load_echap_yaml(&fixture("ioc.yaml"), "echap/ioc", FeedClass::Stalkerware).unwrap();
    let raw = ioc.indicators.len();
    let watch =
        load_echap_yaml(&fixture("watchware.yaml"), "echap/watchware", FeedClass::Watchware)
            .unwrap();
    let set = IndicatorSet::from_feeds(vec![ioc, watch]);
    // Same family lists copy9.com as both website and C2 → deduped, max severity wins.
    assert!(set.len() < raw + 159);
    let copy9: Vec<_> = set
        .indicators
        .iter()
        .filter(|i| i.value == "copy9.com" && i.malware == "TheTruthSpy")
        .collect();
    assert_eq!(copy9.len(), 1);
    assert_eq!(copy9[0].severity, Severity::Warning);
}

#[test]
fn synthetic_bundle_skips_unknown_patterns_without_failing() {
    let synthetic = r#"{
      "type": "bundle", "id": "bundle--1",
      "objects": [
        {"type": "malware", "id": "malware--1", "name": "TestWare"},
        {"type": "indicator", "id": "indicator--1",
         "pattern": "[app:id = 'Com.Evil.APP']"},
        {"type": "indicator", "id": "indicator--2",
         "pattern": "[x509-certificate:serial_number = '00']"},
        {"type": "indicator", "id": "indicator--3",
         "pattern": "[url:value = 'https://evil.example/a' OR url:value = 'https://evil.example/b']"},
        {"type": "indicator", "id": "indicator--4"},
        {"type": "relationship", "id": "relationship--1",
         "source_ref": "indicator--1", "target_ref": "malware--1",
         "relationship_type": "indicates"}
      ]
    }"#;
    let feed = load_stix_bundle(synthetic, "test/synthetic", FeedClass::Stalkerware).unwrap();
    // app:id parsed + normalized; two URLs from the OR pattern.
    assert_eq!(feed.indicators.len(), 3);
    let bundle: Vec<_> = feed
        .indicators
        .iter()
        .filter(|i| i.kind == IndicatorKind::BundleId)
        .collect();
    assert_eq!(bundle.len(), 1);
    assert_eq!(bundle[0].value, "com.evil.app");
    assert_eq!(bundle[0].malware, "TestWare");
    assert_eq!(bundle[0].severity, Severity::Critical);
    // Unknown path + patternless indicator both reported, not fatal.
    assert_eq!(feed.skipped.len(), 2);
    assert!(feed.skipped.iter().any(|s| s.contains("x509-certificate")));
    // Attribution falls back to the single malware object for unrelated
    // indicators (indicator--3 has no relationship).
    assert!(feed
        .indicators
        .iter()
        .filter(|i| i.kind == IndicatorKind::Url)
        .all(|i| i.malware == "TestWare"));
}

#[test]
fn bundled_snapshot_loads_offline() {
    let dir = traceloupe_core::indicators::bundled_snapshot_dir();
    let (set, info) = traceloupe_core::indicators::load_snapshot_dir(&dir).expect("snapshot loads");
    // 11 mercenary STIX bundles + Echap ioc.yaml + watchware.yaml.
    assert_eq!(info.feeds.len(), 13);
    assert!(!info.generated_at.is_empty());
    // Every feed parsed (a failed feed reports count 0).
    for f in &info.feeds {
        assert!(f.count > 0, "feed {} loaded nothing", f.source);
    }
    // The merged set is substantial and covers the kinds the scan engine needs.
    assert!(set.len() > 5000, "only {} indicators", set.len());
    for kind in [
        IndicatorKind::Domain,
        IndicatorKind::ProcessName,
        IndicatorKind::BundleId,
        IndicatorKind::Email,
    ] {
        assert!(set.count_by_kind(kind) > 0, "no {kind:?} indicators");
    }
    // Attribution must ship next to the data.
    assert!(dir.join("ATTRIBUTION.md").exists());
}

#[test]
fn malformed_feed_is_a_feed_error() {
    assert!(load_stix_bundle("not json", "x", FeedClass::Mercenary).is_err());
    assert!(load_echap_yaml(": broken\n- yaml", "x", FeedClass::Stalkerware).is_err());
}
