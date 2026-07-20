//! Feed-fetch tests: a throwaway local HTTP server stands in for the public
//! feed repos, so the test verifies the download → swap-into-place → load
//! round-trip without touching the network.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use traceloupe_core::indicators::{
    active_snapshot_dir, fetch_snapshot_with, load_snapshot_dir,
};

/// Serve a fixed set of `path -> body` responses until `count` requests have
/// been handled, on a throwaway port. Returns the base URL.
fn serve(routes: Vec<(String, Vec<u8>)>, count: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..count {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut req_line = String::new();
            reader.read_line(&mut req_line).unwrap();
            // "GET /path HTTP/1.1"
            let path = req_line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                if line == "\r\n" {
                    break;
                }
                line.clear();
            }
            let body = routes
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
            stream.flush().unwrap();
        }
    });
    base
}

/// A minimal two-feed snapshot dir whose manifest points feed URLs at `base`.
fn bundled_with_urls(dir: &std::path::Path, base: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let ioc = "- name: DemoWare\n  type: stalkerware\n  packages: [com.demo.spy]\n  c2: {domains: [c2.demo.example]}\n";
    let stix = r#"{"type":"bundle","objects":[
        {"type":"malware","id":"malware--1","name":"DemoSpy"},
        {"type":"indicator","id":"indicator--1","pattern":"[domain-name:value = 'evil.demo.example']"}
    ]}"#;
    std::fs::write(dir.join("ioc.yaml"), ioc).unwrap();
    std::fs::write(dir.join("demo.stix2"), stix).unwrap();
    let manifest = format!(
        r#"{{"generated_at":"2000-01-01T00:00:00Z","feeds":[
            {{"file":"demo.stix2","source":"demo/spy","class":"mercenary","format":"stix2","url":"{base}/demo.stix2"}},
            {{"file":"ioc.yaml","source":"echap/ioc","class":"stalkerware","format":"echap_yaml","url":"{base}/ioc.yaml"}}
        ]}}"#
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    std::fs::write(dir.join("ATTRIBUTION.md"), "attribution").unwrap();
}

#[test]
fn fetch_downloads_swaps_and_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let bundled = tmp.path().join("bundled");
    let dest = tmp.path().join("appdata/indicators");

    // Bundled snapshot ships an OLD value; the server serves a NEW one.
    bundled_with_urls(&bundled, "PLACEHOLDER");
    let base = serve(
        vec![
            (
                "/demo.stix2".to_string(),
                br#"{"type":"bundle","objects":[
                    {"type":"malware","id":"malware--1","name":"DemoSpy"},
                    {"type":"indicator","id":"indicator--1","pattern":"[domain-name:value = 'fresh.demo.example']"}
                ]}"#
                .to_vec(),
            ),
            (
                "/ioc.yaml".to_string(),
                b"- name: DemoWare\n  type: stalkerware\n  packages: [com.demo.spy, com.demo.spy2]\n".to_vec(),
            ),
        ],
        2,
    );
    // Rewrite the bundled manifest URLs to the live server.
    bundled_with_urls(&bundled, &base);

    let agent = ureq::AgentBuilder::new().build();
    let mut seen = Vec::new();
    let info = fetch_snapshot_with(&agent, &bundled, &dest, |file, _, _| {
        seen.push(file.to_string())
    })
    .unwrap();

    assert_eq!(seen.len(), 2);
    // generated_at was re-stamped to fetch time (no longer the 2000 epoch).
    assert!(!info.generated_at.starts_with("2000"));

    let (set, _) = load_snapshot_dir(&dest).unwrap();
    // Fresh domain replaced the bundled one; new package landed.
    assert!(set.indicators.iter().any(|i| i.value == "fresh.demo.example"));
    assert!(set.indicators.iter().any(|i| i.value == "com.demo.spy2"));
    // Attribution copied alongside.
    assert!(dest.join("ATTRIBUTION.md").exists());
}

#[test]
fn active_dir_prefers_fetched_over_bundled() {
    let tmp = tempfile::tempdir().unwrap();
    let app_data = tmp.path().join("appdata");
    let bundled = tmp.path().join("bundled");
    std::fs::create_dir_all(&bundled).unwrap();

    // No fetched snapshot yet → bundled.
    assert_eq!(active_snapshot_dir(&app_data, &bundled), bundled);

    // A fetched snapshot with a manifest → that wins.
    let fetched = app_data.join("indicators");
    std::fs::create_dir_all(&fetched).unwrap();
    std::fs::write(fetched.join("manifest.json"), "{}").unwrap();
    assert_eq!(active_snapshot_dir(&app_data, &bundled), fetched);
}

#[test]
fn failed_fetch_leaves_previous_snapshot_intact() {
    let tmp = tempfile::tempdir().unwrap();
    let bundled = tmp.path().join("bundled");
    let dest = tmp.path().join("indicators");
    bundled_with_urls(&bundled, "http://127.0.0.1:9"); // nothing listening

    // Seed dest with a prior good snapshot.
    bundled_with_urls(&dest, "unused");
    let before = std::fs::read_to_string(dest.join("demo.stix2")).unwrap();

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_millis(200))
        .build();
    let err = fetch_snapshot_with(&agent, &bundled, &dest, |_, _, _| {});
    assert!(err.is_err());
    // The old feed file is untouched (temp-then-rename never overwrote it).
    assert_eq!(std::fs::read_to_string(dest.join("demo.stix2")).unwrap(), before);
}
