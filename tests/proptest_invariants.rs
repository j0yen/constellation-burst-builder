//! Property-based invariant tests.
//!
//! Read-only after scaffold. The edit-agent must NOT modify proptests.

use proptest::prelude::*;
use wm_burst::cost::CostLog;
use wm_burst::toolchain::{check_toolchain_match, ToolchainChannel};

proptest! {
    /// Budget invariant: month-to-date spend is always >= 0.
    #[test]
    fn budget_never_negative(costs in proptest::collection::vec(0.0_f64..10000.0, 0..20)) {
        let mut log = CostLog::default();
        for cost in costs {
            let now = chrono::Utc::now();
            log.entries_mut().push(wm_burst::cost::JobEntry {
                job_id: "proptest".into(),
                ran_on: "pod:p1".into(),
                started_at: now,
                ended_at: Some(now),
                elapsed_secs: Some(1.0),
                cost_usd: cost,
                description: "proptest".into(),
            });
        }
        prop_assert!(log.month_to_date_usd() >= 0.0);
    }

    /// Toolchain invariant: matching a version against itself always succeeds.
    #[test]
    fn toolchain_self_match_always_passes(
        major in 1_u32..=2,
        minor in 0_u32..=100,
        patch in 0_u32..=10,
    ) {
        let version = format!("{major}.{minor}.{patch}");
        let channel = ToolchainChannel(version.clone());
        let rustc_output = format!("rustc {version} (fakehash 2025-01-01)");
        prop_assert!(check_toolchain_match(&channel, &rustc_output).is_ok());
    }
}
