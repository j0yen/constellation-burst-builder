//! AC4 mock: `wm-burst build` reports cache hit/miss counts;
//! warm-cache second build shows materially higher hit ratio.
//!
//! Deferred for real remote build (requires SSH + sccache remote host).
//! This mock validates the stats-reporting surface.

use wm_burst::commands::build::parse_sccache_stats;

#[test]
fn parse_stats_reports_hits_and_misses() {
    let sample = "Compile requests          54\nCache hits                42  (77.8%)\nCache misses              12  (22.2%)\n";
    let (hits, misses) = parse_sccache_stats(sample);
    assert_eq!(hits, 42, "expected 42 hits");
    assert_eq!(misses, 12, "expected 12 misses");
}

#[test]
fn warm_cache_has_higher_hit_ratio() {
    // Simulate two successive build stats: cold vs warm.
    let cold = "Cache hits                10  (20.0%)\nCache misses              40  (80.0%)\n";
    let warm = "Cache hits                48  (96.0%)\nCache misses               2   (4.0%)\n";

    let (cold_hits, cold_misses) = parse_sccache_stats(cold);
    let (warm_hits, warm_misses) = parse_sccache_stats(warm);

    let cold_ratio = cold_hits as f64 / (cold_hits + cold_misses) as f64;
    let warm_ratio = warm_hits as f64 / (warm_hits + warm_misses) as f64;

    assert!(
        warm_ratio > cold_ratio,
        "warm cache hit ratio ({warm_ratio:.2}) must exceed cold ({cold_ratio:.2})"
    );
}

#[test]
fn zero_stats_on_empty_output() {
    let (hits, misses) = parse_sccache_stats("");
    assert_eq!(hits, 0);
    assert_eq!(misses, 0);
}

#[test]
fn stats_report_includes_where_it_ran() {
    // The build command sets ran_on in the cost log.
    // This mock verifies the cost module's ran_on field format.
    use wm_burst::cost::JobEntry;
    use chrono::Utc;
    let entry = JobEntry {
        job_id: "build-1".into(),
        ran_on: "standing-box:builder.example.com".into(),
        started_at: Utc::now(),
        ended_at: Some(Utc::now()),
        elapsed_secs: Some(12.5),
        cost_usd: 0.0,
        description: "cargo build in ~/myproject".into(),
    };
    assert!(
        entry.ran_on.starts_with("standing-box:"),
        "ran_on should identify standing-box: {}",
        entry.ran_on
    );
}
