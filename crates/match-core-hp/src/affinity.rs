//! Optional CPU pinning (DPDK / Aeron deployment practice).
//!
//! Enable with `--features affinity` and link against a platform that supports
//! [`core_affinity`]. Without the feature, [`pin_current_thread`] returns an error.
//!
//! Typical use: spawn one OS thread per symbol worker, then pin before `poll`.

/// Error when pinning is unavailable or the core id is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffinityError(pub &'static str);

impl std::fmt::Display for AffinityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AffinityError {}

/// Pin the **current** thread to `core_id` (logical CPU index).
///
/// Requires crate feature `affinity`.
pub fn pin_current_thread(core_id: usize) -> Result<(), AffinityError> {
    #[cfg(feature = "affinity")]
    {
        let cores = core_affinity::get_core_ids().ok_or(AffinityError("no core ids"))?;
        let core = cores
            .into_iter()
            .find(|c| c.id == core_id)
            .ok_or(AffinityError("core_id not found"))?;
        if core_affinity::set_for_current(core) {
            Ok(())
        } else {
            Err(AffinityError("set_for_current failed"))
        }
    }
    #[cfg(not(feature = "affinity"))]
    {
        let _ = core_id;
        Err(AffinityError(
            "rebuild with `--features affinity` to enable CPU pinning",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_feature_returns_clear_error() {
        #[cfg(not(feature = "affinity"))]
        {
            let err = pin_current_thread(0).unwrap_err();
            assert!(err.0.contains("affinity"));
        }
        #[cfg(feature = "affinity")]
        {
            // May succeed or fail depending on OS permissions; just ensure call is safe.
            let _ = pin_current_thread(0);
        }
    }
}
