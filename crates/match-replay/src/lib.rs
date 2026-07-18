//! GoldenTrace NDJSON replay: run `match-core::Engine` on input ops and diff fill/depth/revoke.
//!
//! See [`trace`] for the input order format (simplified BbOrder test shape).

pub mod diff;
pub mod trace;

pub use diff::{decimals_equal, diff_outcomes, diff_replay};
pub use trace::{
    collect_replay, load_ndjson, replay_path, replay_paths, snapshot_depth, OutcomeEvent,
    ReplayCollected, SimplifiedOrder, TraceLine,
};

use std::path::{Path, PathBuf};

/// Run replay+diff for one golden file. `Ok(())` on match.
pub fn assert_golden_matches(path: &Path) -> Result<(), Vec<String>> {
    let collected = replay_path(path).map_err(|e| vec![e])?;
    let diffs = diff_replay(&collected);
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(diffs)
    }
}

/// Directory containing `*.ndjson` golden files (workspace `testdata/golden`).
pub fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden")
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use std::fs;

    #[test]
    fn all_golden_ndjson_match() {
        let dir = golden_dir();
        assert!(dir.is_dir(), "golden dir missing: {}", dir.display());
        let mut files: Vec<_> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("ndjson"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no *.ndjson under {}", dir.display());

        let mut failures = Vec::new();
        for path in &files {
            match assert_golden_matches(path) {
                Ok(()) => {}
                Err(diffs) => {
                    failures.push(format!("{}:\n  {}", path.display(), diffs.join("\n  ")));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "golden mismatches:\n{}",
            failures.join("\n")
        );
    }
}
