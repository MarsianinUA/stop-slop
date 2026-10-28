//! Values of the file: CPUs, bytes, durations, counts and percents. A refusal is a message; the
//! field the text came from puts in the key and the line.

use std::num::NonZeroU32;
use std::time::Duration;

use crate::{Bytes, Cpus, Percent};

/// `"0.5"`, `0.5` or `2`: thousandths of a CPU, above 0.
pub(crate) fn cpus(text: &str) -> Result<Cpus, String> {
    let refused = || format!("`{text}` is not a number of CPUs such as `0.5` or `2`, above 0");
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !digits(whole, fraction) {
        return Err(refused());
    }
    let thousandths = format!("{whole}{fraction:0<3}")
        .parse::<u32>()
        .map_err(|_| refused())?;
    match thousandths {
        0 => Err(refused()),
        thousandths => Ok(Cpus(thousandths)),
    }
}

/// Bytes, or a number with `b`, `k`, `m`, `g`, `t` or `p`, which are powers of 1024 as in
/// Docker's `RAMInBytes`: `512m`, `1.5g`, `1GiB`, `1073741824`. Above 0.
pub(crate) fn bytes(text: &str) -> Result<Bytes, String> {
    let refused = || format!("`{text}` is not a size such as `512m`, `1g` or a number of bytes");
    let lower = text.to_ascii_lowercase();
    let rest = lower.strip_suffix('b').unwrap_or(&lower);
    let rest = rest.strip_suffix('i').unwrap_or(rest);
    let unit = rest
        .char_indices()
        .last()
        .and_then(|(at, unit)| Some((at, "kmgtp".find(unit)?)));
    let (number, power) = match unit {
        Some((at, index)) => (&rest[..at], index as u32 + 1),
        None => (rest, 0),
    };
    let number = number.strip_suffix(' ').unwrap_or(number);
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    // 20 digits times 1024^5 stay within u128.
    if !digits(whole, fraction) || whole.len() + fraction.len() > 20 {
        return Err(refused());
    }
    let scaled = format!("{whole}{fraction}")
        .parse::<u128>()
        .map_err(|_| refused())?
        * 1024u128.pow(power)
        / 10u128.pow(fraction.len() as u32);
    match u64::try_from(scaled) {
        Ok(0) | Err(_) => Err(refused()),
        Ok(bytes) => Ok(Bytes(bytes)),
    }
}

/// A Compose duration such as `1m30s` or `500ms`, read by `humantime`.
pub(crate) fn duration(text: &str) -> Result<Duration, String> {
    humantime::parse_duration(text)
        .map_err(|error| format!("`{text}` is not a duration such as `1m30s` or `500ms`: {error}"))
}

/// A `duration` above 0.
pub(crate) fn period(text: &str) -> Result<Duration, String> {
    match duration(text)? {
        Duration::ZERO => Err(format!("`{text}` is not above 0")),
        period => Ok(period),
    }
}

/// A whole number from 1.
pub(crate) fn count(text: &str) -> Result<NonZeroU32, String> {
    is_whole(text)
        .then(|| text.parse::<NonZeroU32>().ok())
        .flatten()
        .ok_or_else(|| format!("`{text}` is not a whole number from 1"))
}

pub(crate) fn percent(value: u16) -> Option<Percent> {
    (1..=1000).contains(&value).then_some(Percent(value))
}

/// Digits only: Rust's `parse` also takes a leading `+`.
pub(crate) fn is_whole(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Both parts are digits, and not both are empty: `2`, `0.5`, `.5`.
fn digits(whole: &str, fraction: &str) -> bool {
    let all = |part: &str| part.is_empty() || is_whole(part);
    all(whole) && all(fraction) && !(whole.is_empty() && fraction.is_empty())
}
