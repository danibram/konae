//! An action's `schedule` and `subscribe` keys, copied from
//! kernel-rs/src/schedule.rs at 8ecb7e5. The timer and the clock stay in the
//! Kernel.

use super::refusal::{refuse, Refusal};
use super::ActionDefinition;

/// The shortest period a declaration may name, in minutes.
pub(crate) const MIN_MINUTES: u64 = 5;
/// The longest period, one day.
pub(crate) const MAX_MINUTES: u64 = 24 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Schedule {
    Every { minutes: u64 },
    Daily { minute_of_day: u64 },
}

/// Parses and checks one action's `schedule` key. `Ok(None)` when it has none.
pub(crate) fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<Schedule>, Refusal> {
    let Some(value) = action.schedule.as_ref() else { return Ok(None) };
    // Each message is written out so src/sdk/contract.test.ts finds it.
    if action.kind == "query" {
        return Err(refuse("schedule.not-mutation", format!("Action {name} is a query, so it cannot declare a schedule. Declare it on a mutation.")));
    }
    if action.kind != "mutation" {
        return Err(refuse("schedule.not-mutation", format!("Action {name} is a signal action, so it cannot declare a schedule. Declare it on a mutation.")));
    }
    if action.bindings.iter().any(|binding| binding.source == "input") {
        return Err(refuse("schedule.input", format!(
            "Action {name} declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values."
        )));
    }
    let invalid = || {
        refuse("schedule.invalid", format!(r#"Action {name} has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00"."#))
    };
    let record = value.as_object().filter(|record| record.len() == 1).ok_or_else(invalid)?;
    let (key, text) = record.iter().next().map(|(key, text)| (key.as_str(), text.as_str())).ok_or_else(invalid)?;
    let text = text.ok_or_else(invalid)?;
    match key {
        "every" => {
            let (digits, unit) = text.split_at(text.len().saturating_sub(1));
            let well_formed = (1..=5).contains(&digits.len())
                && !digits.starts_with('0')
                && digits.bytes().all(|byte| byte.is_ascii_digit())
                && (unit == "m" || unit == "h");
            if !well_formed {
                return Err(invalid());
            }
            let minutes = digits.parse::<u64>().map_err(|_| invalid())? * if unit == "h" { 60 } else { 1 };
            if minutes < MIN_MINUTES {
                return Err(refuse("schedule.too-often", format!(r#"Action {name} runs every {text}, more often than every 5 minutes. Use "5m" or longer."#)));
            }
            if minutes > MAX_MINUTES {
                return Err(refuse("schedule.too-rare", format!(r#"Action {name} runs every {text}, less often than once a day. Use "daily" or a period of 24 hours or less."#)));
            }
            Ok(Some(Schedule::Every { minutes }))
        }
        "daily" => {
            let bytes = text.as_bytes();
            let digit = |index: usize| bytes.get(index).filter(|byte| byte.is_ascii_digit()).map(|byte| u64::from(byte - b'0'));
            let time = match (bytes.len(), bytes.get(2), digit(0), digit(1), digit(3), digit(4)) {
                (5, Some(b':'), Some(h1), Some(h2), Some(m1), Some(m2)) => Some((h1 * 10 + h2, m1 * 10 + m2)),
                _ => None,
            };
            match time {
                Some((hour, minute)) if hour < 24 && minute < 60 => Ok(Some(Schedule::Daily { minute_of_day: hour * 60 + minute })),
                _ => Err(refuse("schedule.daily-invalid", format!("Action {name} has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59."))),
            }
        }
        _ => Err(invalid()),
    }
}

/// Refuses `subscribe` on any action: the Kernel pushes nothing to a page.
pub(crate) fn check_subscribe(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if action.subscribe.is_some() {
        return Err(refuse("action.subscribe-unsupported", format!(
            "Action {name} declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead."
        )));
    }
    Ok(())
}
