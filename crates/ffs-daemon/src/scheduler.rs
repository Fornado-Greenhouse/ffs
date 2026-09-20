//! Auditor scheduler (task_41): drives the auditor skill's `tick` (the
//! daily health summary) and `briefing` (the morning briefing) on
//! their own intervals from inside the daemon, so a Mac that is only
//! ever opened in the morning still has a fresh briefing waiting.
//!
//! Intervals come from `FFS_AUDITOR_TICK_INTERVAL` (default `24h`) and
//! `FFS_AUDITOR_BRIEFING_INTERVAL` (default `7d`), written as
//! `<n><unit>` with unit `s`, `m`, `h`, or `d`. `0` or `off` disables
//! that schedule. The first run happens one interval after boot, not at
//! boot: the daemon may be restarted many times a day and the auditor
//! should not publish a summary on every restart. `audit.run` exists
//! for "now, please".

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::dispatch::SkillInvoker;

pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
pub const DEFAULT_BRIEFING_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Parse `<n><unit>` (`30s`, `15m`, `24h`, `7d`) or a bare number of
/// seconds. `0`, `off`, `none`, and an empty string mean disabled
/// (`Ok(None)`). Anything else is an error naming the input.
pub fn parse_interval(raw: &str) -> Result<Option<Duration>, String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() || s == "off" || s == "none" || s == "0" {
        return Ok(None);
    }
    let (digits, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => s.split_at(i),
        None => (s.as_str(), "s"),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("interval `{raw}`: expected <number><s|m|h|d>"))?;
    let mult = match unit {
        "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" => 60 * 60,
        "d" | "day" | "days" => 24 * 60 * 60,
        other => return Err(format!("interval `{raw}`: unknown unit `{other}`")),
    };
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(Duration::from_secs(n * mult)))
}

/// One schedule: invoke the auditor with `{"op": op}` every `every`,
/// until `cancel` fires. Failures are logged and the schedule keeps
/// going (the next run may succeed once the skills host has recovered).
pub fn spawn_schedule(
    invoker: Arc<dyn SkillInvoker>,
    op: &'static str,
    every: Duration,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` fires immediately on first tick; skip it so a
        // restart does not publish.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => {}
            }
            match invoker.invoke("auditor", json!({"op": op})).await {
                Ok(v) => info!(op, result = %v, "auditor schedule ran"),
                Err(e) => warn!(op, error = %e, "auditor schedule failed"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units_and_bare_seconds() {
        assert_eq!(
            parse_interval("30s").unwrap(),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_interval("15m").unwrap(),
            Some(Duration::from_secs(900))
        );
        assert_eq!(
            parse_interval("24h").unwrap(),
            Some(Duration::from_secs(86_400))
        );
        assert_eq!(
            parse_interval("7d").unwrap(),
            Some(Duration::from_secs(604_800))
        );
        assert_eq!(
            parse_interval("1D").unwrap(),
            Some(Duration::from_secs(86_400))
        );
        assert_eq!(parse_interval("90").unwrap(), Some(Duration::from_secs(90)));
    }

    #[test]
    fn zero_off_and_empty_disable() {
        for raw in ["0", "0d", "off", "none", "", "  "] {
            assert_eq!(parse_interval(raw).unwrap(), None, "{raw:?}");
        }
    }

    #[test]
    fn garbage_is_an_error_naming_the_input() {
        let err = parse_interval("soon").unwrap_err();
        assert!(err.contains("soon"), "{err}");
        let err = parse_interval("3 fortnights").unwrap_err();
        assert!(
            err.contains("fortnights") || err.contains("3 fortnights"),
            "{err}"
        );
    }
}
