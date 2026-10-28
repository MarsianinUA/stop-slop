//! The top-level `x-stand` keys beyond Compose: probes, the declared cost, the fault and the
//! second pass, and the grammar of a change.

use std::collections::BTreeMap;

use crate::bind::{name, optional, required, service_named};
use crate::tree::Field;
use crate::{
    Cost, Fault, FaultMode, Invalid, Percent, Pointer, Policy, Probe, ProbeName, SecondPass,
    Service, ServiceKind, ServiceName, Tap, Target, Unchanged, graph, units,
};

/// The smallest `X` of `+X%` and `-X%`: a smaller shift is within noise, which `none` says.
const MIN_CHANGE: u32 = 5;

/// `none`, `+X%`, `-X%`, `down`, `unknown`; X a whole percent from 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    UpTo(Percent),
    DownTo(Percent),
    Down,
    Unknown,
}

impl Change {
    /// The grammar of `x-stand.expect.cost[].change` and of `accept --allow`.
    pub fn parse(text: &str) -> Result<Change, ChangeError> {
        let refused = |message: &str| ChangeError {
            text: text.to_owned(),
            message: message.to_owned(),
        };
        let change: fn(Percent) -> Change = match text {
            "none" => return Ok(Change::None),
            "down" => return Ok(Change::Down),
            "unknown" => return Ok(Change::Unknown),
            _ if text.starts_with('+') => Change::UpTo,
            _ if text.starts_with('-') => Change::DownTo,
            _ => return Err(refused("no sign")),
        };
        let digits = text[1..]
            .strip_suffix('%')
            .ok_or_else(|| refused("no `%`"))?;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(refused("X is not a whole percent"));
        }
        let x = digits.parse::<u32>().unwrap_or(u32::MAX);
        if x < MIN_CHANGE {
            return Err(refused("X is below 5, a shift within noise"));
        }
        let percent = units::percent(x).ok_or_else(|| refused("X is above 1000"))?;
        Ok(change(percent))
    }

    /// `UpTo` and `DownTo` no wider than `max_tolerance`; the others always pass.
    pub fn within(self, policy: &Policy) -> bool {
        match self {
            Change::UpTo(percent) | Change::DownTo(percent) => {
                percent.get() <= policy.max_tolerance.get()
            }
            Change::None | Change::Down | Change::Unknown => true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("`{text}`: {message}; write none, +X%, -X%, down or unknown, with X from 5")]
pub struct ChangeError {
    pub text: String,
    pub message: String,
}

/// A probe reads the state below the app inside a service that is up: a long-running one, and
/// for `after-setup` one that setup starts.
pub(crate) fn probes(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
) -> Result<BTreeMap<ProbeName, Probe>, Invalid> {
    let in_setup = graph::in_setup(services);
    let mut probes = BTreeMap::new();
    for (probe, field) in root.at("x-stand").at("probes").entries()? {
        let probe = ProbeName(name(&field, probe)?);
        if !field.is_map() {
            return Err(field.invalid("write `{in, at, run, key, ignore}`"));
        }
        let within = required(&field, "in", "the long-running service the probe runs in")?;
        let service = service_named(services, &within)?;
        let kind = match services[&service].kind {
            ServiceKind::LongRunning => None,
            ServiceKind::Setup => Some("a setup service"),
            ServiceKind::Workload => Some("the workload"),
        };
        if let Some(kind) = kind {
            return Err(within.invalid(format!(
                "`{}` is {kind}: a probe runs in a long-running service, which is up when it reads",
                service.as_str()
            )));
        }
        let at_field = required(&field, "at", "after-setup, after-workload or on-failure")?;
        let at = at_field.parse(tap)?;
        if at == Tap::AfterSetup && !in_setup.contains(&service) {
            return Err(at_field.invalid(format!(
                "`{}` does not run in setup, where `after-setup` probes: no setup service waits \
                 for it",
                service.as_str()
            )));
        }
        let run = required(&field, "run", "the argv of the probe")?;
        let argv = run.strings()?;
        if argv.is_empty() {
            return Err(run.invalid("an empty argv"));
        }
        let ignore = optional(&field, "ignore", |field| {
            field
                .items()?
                .iter()
                .map(|item| item.parse(pointer))
                .collect()
        })?;
        probes.insert(
            probe,
            Probe {
                service,
                at,
                run: argv,
                key: optional(&field, "key", |field| field.parse(pointer))?,
                ignore: ignore.unwrap_or_default(),
            },
        );
    }
    Ok(probes)
}

fn tap(text: &str) -> Result<Tap, String> {
    match text {
        "after-setup" => Ok(Tap::AfterSetup),
        "after-workload" => Ok(Tap::AfterWorkload),
        "on-failure" => Ok(Tap::OnFailure),
        _ => Err(format!(
            "`{text}` is not `after-setup`, `after-workload` or `on-failure`"
        )),
    }
}

fn pointer(text: &str) -> Result<Pointer, String> {
    let escaped = text
        .match_indices('~')
        .all(|(at, _)| matches!(text.as_bytes().get(at + 1), Some(b'0' | b'1')));
    if (text.is_empty() || text.starts_with('/')) && escaped {
        Ok(Pointer(text.to_owned()))
    } else {
        Err(format!(
            "`{text}` is not a JSON pointer such as `/id` or `/items/0/sku`"
        ))
    }
}

pub(crate) fn expect(root: &Field, policy: &Policy) -> Result<Vec<Cost>, Invalid> {
    let cost = root.at("x-stand").at("expect").at("cost");
    cost.items()?
        .iter()
        .map(|item| self::cost(item, policy))
        .collect()
}

fn cost(item: &Field, policy: &Policy) -> Result<Cost, Invalid> {
    if !item.is_map() {
        return Err(item.invalid("write `{measure, change, reason}`"));
    }
    let measure = required(item, "measure", "the measure, such as `stand.wall`")?.string()?;
    let field = required(item, "change", "none, +X%, -X%, down or unknown")?;
    let text = field.string()?;
    let change = Change::parse(&text).map_err(|error| field.invalid(error.to_string()))?;
    if !change.within(policy) {
        return Err(field.invalid(format!(
            "`{text}` is wider than `max_tolerance` of stand.toml, {}%",
            policy.max_tolerance.get()
        )));
    }
    Ok(Cost {
        measure,
        change,
        reason: reason(item)?,
    })
}

/// A reason is never empty: the cost is declared before the run, with why.
fn reason(item: &Field) -> Result<String, Invalid> {
    let field = required(item, "reason", "why it changes so")?;
    let reason = field.string()?;
    if reason.trim().is_empty() {
        return Err(field.invalid("empty: say why it changes so"));
    }
    Ok(reason)
}

pub(crate) fn fault(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
) -> Result<Option<Fault>, Invalid> {
    let Some(field) = root.at("x-stand").get("fault") else {
        return Ok(None);
    };
    if !field.is_map() {
        return Err(field.invalid("write `{target: <service>:<port>, mode: down}`"));
    }
    let target = target(
        &required(&field, "target", "`<service>:<port>`, such as `db:5432`")?,
        services,
    )?;
    let mode_field = required(&field, "mode", "down or slow")?;
    let mode = match mode_field.string()?.as_str() {
        "down" => match field.get("delay") {
            Some(delay) => return Err(delay.invalid("`delay` is for `mode: slow`")),
            None => FaultMode::Down,
        },
        "slow" => FaultMode::Slow(
            required(&field, "delay", "how long `slow` holds each connection")?
                .parse(units::period)?,
        ),
        mode => {
            return Err(mode_field.invalid(format!("`{mode}` is not `down` or `slow`")));
        }
    };
    Ok(Some(Fault { target, mode }))
}

/// The relay reaches a service by its `expose` ports, so the target is one of them.
fn target(field: &Field, services: &BTreeMap<ServiceName, Service>) -> Result<Target, Invalid> {
    let text = field.string()?;
    let Some((service, port)) = text.rsplit_once(':') else {
        return Err(field.invalid(format!("`{text}` is not `<service>:<port>`")));
    };
    let found = services.iter().find(|(known, _)| known.as_str() == service);
    let Some((known, definition)) = found else {
        return Err(field.invalid(format!("no service `{service}`")));
    };
    let exposed = definition
        .expose
        .iter()
        .find(|exposed| exposed.get().to_string() == port);
    let Some(&port) = exposed else {
        return Err(field.invalid(format!(
            "`{service}` does not expose `{port}`: the relay reaches a service by its `expose`"
        )));
    };
    Ok(Target {
        service: known.clone(),
        port,
    })
}

/// `x-stand.expect.unchanged` names files the second pass compares, so it needs the second pass.
pub(crate) fn second_pass(root: &Field) -> Result<Option<SecondPass>, Invalid> {
    let x_stand = root.at("x-stand");
    let on = optional(&x_stand, "second_pass", Field::bool)?.unwrap_or(false);
    let unchanged = x_stand.at("expect").get("unchanged");
    match (on, unchanged) {
        (false, None) => Ok(None),
        (false, Some(field)) => Err(field.invalid(
            "`unchanged` needs `x-stand.second_pass: true`: only the second pass runs the \
             workload twice on one state",
        )),
        (true, unchanged) => {
            let items = unchanged.map(|field| field.items()).transpose()?;
            let unchanged = items
                .unwrap_or_default()
                .iter()
                .map(self::unchanged)
                .collect::<Result<_, _>>()?;
            Ok(Some(SecondPass { unchanged }))
        }
    }
}

fn unchanged(item: &Field) -> Result<Unchanged, Invalid> {
    if !item.is_map() {
        return Err(item.invalid("write `{file, reason}`"));
    }
    Ok(Unchanged {
        file: required(item, "file", "a file of /stand/out or `probe.<name>.jsonl`")?.string()?,
        reason: reason(item)?,
    })
}
