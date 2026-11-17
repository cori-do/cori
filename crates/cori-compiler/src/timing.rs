// SPDX-License-Identifier: Apache-2.0
//! The timing a workflow declares, one grammar each: a duration is a whole number and a unit, `ms`, `s`, `m`, `h` or `d`
//! (`30s`, `5m`, `2h`, `3d`: a human step's `timeout`, `endpoint.timeout`, `budget.max_duration`); a rate is a count per
//! unit, optionally per caller (`endpoint.rate_limit: 30/min/principal`); a schedule is a cron expression in a time zone.
//! `cori check` refuses anything else, so the engine and the server read only what it accepted.
use std::time::Duration;

/// `30s`, `5m`, `2h`, `3d`, `500ms` (surrounding spaces and a space before the unit are allowed).
pub fn duration(s: &str) -> Result<Duration, String> {
    let t = s.trim();
    let digits = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(digits);
    if num.is_empty() {
        return Err(format!("`{s}` is not a duration: write a whole number and a unit (ms, s, m, h or d), e.g. `30s` or `2h`"));
    }
    let n: u64 = num.parse().map_err(|_| format!("`{s}` is too long"))?;
    let per_unit: u64 = match unit.trim() {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "" => return Err(format!("`{s}` has no unit: write `{n}s`, `{n}m`, `{n}h` or `{n}d`")),
        other => return Err(format!("`{s}`: `{other}` is not a unit (ms, s, m, h or d)")),
    };
    n.checked_mul(per_unit).map(Duration::from_millis).ok_or_else(|| format!("`{s}` is too long"))
}

/// A rate limit `N/unit[/principal]`: at most N calls per unit (`s`, `min`, `h`, `d`, or their long forms), for all
/// callers together, or per caller with `/principal` (`/user`). Returns (count, window, per caller).
pub fn rate_limit(s: &str) -> Result<(usize, Duration, bool), String> {
    let parts: Vec<String> = s.split('/').map(|p| p.trim().to_ascii_lowercase()).collect();
    let usage = || format!("`{s}` is not a rate limit: write `<count>/<unit>` or `<count>/<unit>/principal`, e.g. `30/min/principal`");
    let (count, unit, per) = match parts.as_slice() {
        [c, u] => (c, u, None),
        [c, u, p] => (c, u, Some(p.as_str())),
        _ => return Err(usage()),
    };
    let count: usize = count.parse().map_err(|_| usage())?;
    if count == 0 { return Err(format!("`{s}`: a limit of 0 refuses every call")); }
    let window = match unit.as_str() {
        "s" | "sec" | "second" => 1,
        "m" | "min" | "minute" => 60,
        "h" | "hour" => 3600,
        "d" | "day" => 86400,
        other => return Err(format!("`{s}`: `{other}` is not a unit (s, min, h or d)")),
    };
    let per_principal = match per {
        None => false,
        Some("principal" | "user") => true,
        Some(other) => return Err(format!("`{s}`: `/{other}` is not a scope (`/principal` limits each caller)")),
    };
    Ok((count, Duration::from_secs(window), per_principal))
}

/// A `schedule:` cron expression: five fields (minute hour day-of-month month day-of-week), or six with seconds first.
pub fn cron(expr: &str) -> Result<cron::Schedule, String> {
    let six = if expr.split_whitespace().count() == 5 { format!("0 {expr}") } else { expr.to_string() };
    six.parse::<cron::Schedule>().map_err(|e| format!("`{expr}` is not a cron expression ({e}): five fields, minute hour day month weekday, e.g. `0 7 * * 1-5`"))
}

/// A `schedule_tz:` IANA time zone (`Europe/Paris`, `UTC`).
pub fn time_zone(tz: &str) -> Result<chrono_tz::Tz, String> {
    tz.parse::<chrono_tz::Tz>().map_err(|_| format!("`{tz}` is not a time zone: an IANA name such as `Europe/Paris` or `UTC`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_have_a_number_and_a_unit() {
        let ms = |s: &str| duration(s).map(|d| d.as_millis() as u64);
        assert_eq!(ms("500ms"), Ok(500));
        assert_eq!(ms("30s"), Ok(30_000));
        assert_eq!(ms(" 5 m "), Ok(300_000));
        assert_eq!(ms("2h"), Ok(7_200_000));
        assert_eq!(ms("3d"), Ok(259_200_000));
        for bad in ["30", "", "s", "1h30m", "PT5M", "5min", "2 hours", "-3s", "1.5h", "99999999999999999999d"] {
            assert!(duration(bad).is_err(), "{bad}");
        }
        assert!(duration("30").unwrap_err().contains("`30s`"), "the error suggests the fix");
    }

    #[test]
    fn rate_limits_have_a_count_a_unit_and_maybe_a_scope() {
        assert_eq!(rate_limit("30/min/principal"), Ok((30, Duration::from_secs(60), true)));
        assert_eq!(rate_limit("3/min"), Ok((3, Duration::from_secs(60), false)));
        assert_eq!(rate_limit("10 / Hour / user"), Ok((10, Duration::from_secs(3600), true)));
        for bad in ["30", "30/5min", "0/min", "x/min", "3/min/team", "3/min/principal/x", "3/fortnight"] {
            assert!(rate_limit(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn schedules_are_cron_expressions_in_a_time_zone() {
        assert!(cron("0 7 * * 1-5").is_ok());
        assert!(cron("0 0 7 * * 1-5").is_ok());
        assert!(cron("every morning").is_err());
        assert!(cron("0 25 * * *").is_err());
        assert_eq!(time_zone("Europe/Paris"), Ok(chrono_tz::Europe::Paris));
        assert!(time_zone("Mars/Olympus").is_err());
    }
}
