//! Full pipeline on the committed public fixtures (tests/fixtures/public):
//! scan with the real vrfkit, store, rescan, detect `me`, render exports and
//! trends, and report a corrupt file.
//!
//! Needs the vrfkit `vrfkit.json` names: `$VANTAGE_VRFKIT`, else `vrfkit.exe`
//! on PATH. Skips (passes with a note) without it.

use std::path::PathBuf;

use vantage_core::Catalog;
use vantage_core::scan::{Outcome, ScanOptions, scan};
use vantage_core::store::Store;
use vantage_core::trends::trends;
use vantage_core::vrfkit::{Vrfkit, sha256_file};

const FIXTURES: [(&str, &str); 3] = [
    (
        "9f8b32c5-c243-41ec-bbbb-832582edf652.12_10.vrf",
        "a4ce1b72f9bdf99492162013c1c909e6994a0d22bef1899e687fde71fbc86606",
    ),
    (
        "5c673443-5bdc-4576-b416-aab3f62471a5.12_11.vrf",
        "7a7a5492ddf286bb04413da96f0d3b216f91150e8174a3a4397493529e17ebdd",
    ),
    (
        "12974d2b-848f-490d-80ba-5f03a033c2d5.13_00.vrf",
        "fd49091dd43171bb060eb6bbae50ed6677aa1077344572c5bf65f0c6fe2b4c1a",
    ),
];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/public")
}

fn vrfkit() -> Option<Vrfkit> {
    let configured = std::env::var_os("VANTAGE_VRFKIT").map(PathBuf::from);
    Vrfkit::locate(configured.as_deref(), None).ok()
}

/// A fresh directory under the system temp dir for one test.
fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vantage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn fixtures_are_the_pinned_files() {
    for (name, sha) in FIXTURES {
        assert_eq!(sha256_file(&fixtures_dir().join(name)).unwrap(), sha, "{name}");
    }
}

#[test]
fn scan_public_fixtures() {
    let Some(vrfkit) = vrfkit() else {
        eprintln!("skipping: set VANTAGE_VRFKIT to the vrfkit.exe vrfkit.json names");
        return;
    };
    let data = temp_dir("fixtures");
    let catalog = Catalog::builtin();
    let mut store = Store::open(&data.join("vantage.db")).unwrap();
    let opts = ScanOptions {
        demos_dir: fixtures_dir(),
        data_dir: data.clone(),
        keep_raw: false,
        retry_failed: false,
    };

    // First scan: all three stored, raw exports removed, tracks kept.
    let first = scan(&mut store, &vrfkit, &catalog, &opts, |_| {}).unwrap();
    assert_eq!(first.len(), 3);
    for r in &first {
        assert!(
            matches!(r.outcome, Outcome::Stored { .. }),
            "{}: {:?}",
            r.file,
            r.outcome
        );
    }
    assert!(
        std::fs::read_dir(data.join("raw"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "raw exports are deleted"
    );
    assert_eq!(std::fs::read_dir(data.join("tracks")).unwrap().count(), 3);

    // Second scan: a no-op.
    let second = scan(&mut store, &vrfkit, &catalog, &opts, |_| {}).unwrap();
    assert!(
        second.iter().all(|r| matches!(r.outcome, Outcome::AlreadyStored)),
        "{second:?}"
    );

    // The recorder is the one account in every match.
    let (n, subjects) = store.subjects_in_every_match().unwrap();
    assert_eq!((n, subjects.len()), (3, 1));
    let me = &subjects[0];

    // Every stored review renders, newest builds included, and trends refuses to see patterns.
    let reviews = store.reviews_of(me).unwrap();
    assert_eq!(reviews.len(), 3);
    for r in &reviews {
        let md = r.export_md();
        assert!(md.starts_with("# Death review: "));
        assert!(md.contains("## Rounds table"));
        assert!(!r.round_facts.is_empty());
    }
    let builds: Vec<&str> = reviews.iter().map(|r| r.context.replay_build.as_str()).collect();
    assert!(builds.iter().any(|b| b.ends_with("12.10")) && builds.iter().any(|b| b.ends_with("13.00")));
    let report = trends(me, &reviews, 20);
    assert_eq!(report.ranked().count(), 0);
    assert!(report.markdown().contains("not enough data"));

    std::fs::remove_dir_all(&data).ok();
}

#[test]
fn corrupt_replay_is_reported_once() {
    let Some(vrfkit) = vrfkit() else {
        eprintln!("skipping: set VANTAGE_VRFKIT to the vrfkit.exe vrfkit.json names");
        return;
    };
    let data = temp_dir("corrupt");
    let demos = data.join("demos");
    std::fs::create_dir_all(&demos).unwrap();
    std::fs::write(demos.join("corrupt.vrf"), (0u8..=200).collect::<Vec<u8>>()).unwrap();
    // Scan leaves just-changed replays for later, as they may still be downloading.
    let an_hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(demos.join("corrupt.vrf"))
        .unwrap()
        .set_modified(an_hour_ago)
        .unwrap();
    let catalog = Catalog::builtin();
    let mut store = Store::open(&data.join("vantage.db")).unwrap();
    let mut opts = ScanOptions {
        demos_dir: demos.clone(),
        data_dir: data.clone(),
        keep_raw: false,
        retry_failed: false,
    };

    let first = scan(&mut store, &vrfkit, &catalog, &opts, |_| {}).unwrap();
    assert!(
        matches!(&first[0].outcome, Outcome::Failed { error } if error.contains("vrfkit")),
        "{first:?}"
    );
    let second = scan(&mut store, &vrfkit, &catalog, &opts, |_| {}).unwrap();
    assert!(
        matches!(second[0].outcome, Outcome::PreviouslyFailed { .. }),
        "{second:?}"
    );
    opts.retry_failed = true;
    let third = scan(&mut store, &vrfkit, &catalog, &opts, |_| {}).unwrap();
    assert!(matches!(third[0].outcome, Outcome::Failed { .. }));
    assert_eq!(store.counts().unwrap().matches, 0);

    std::fs::remove_dir_all(&data).ok();
}
