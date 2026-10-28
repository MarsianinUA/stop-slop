//! The top-level `x-stand` keys beyond Compose: probes, the declared cost, the fault and the
//! second pass, and the grammar of a change.

use std::collections::BTreeMap;

use crate::bind::{self, name, optional, required, service_named};
use crate::tree::Field;
use crate::{
    Cost, Fault, FaultMode, Invalid, Percent, Pointer, Policy, Probe, ProbeName, SecondPass,
    Service, ServiceKind, ServiceName, Tap, Target, Unchanged, graph, units,
};

/// The smallest `X` of `+X%` and `-X%`: a smaller shift is within noise, which `none` says.
const MIN_CHANGE: u16 = 5;

/// The largest `X` of `-X%`: a measure cannot fall by more than all of it.
const MAX_FALL: u16 = 100;

/// `none`, `+X%`, `-X%`, `down`, `unknown`; X a whole percent from 5, at most 100 in `-X%`.
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
        if !units::is_whole(digits) {
            return Err(refused("X is not a whole percent"));
        }
        let x = digits.parse::<u16>().unwrap_or(u16::MAX);
        if x < MIN_CHANGE {
            return Err(refused("X is below 5, a shift within noise"));
        }
        if text.starts_with('-') && x > MAX_FALL {
            return Err(refused(
                "X is above 100, and a measure falls by 100% at most",
            ));
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

pub(crate) fn probes(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
) -> Result<BTreeMap<ProbeName, Probe>, Invalid> {
    let in_setup = graph::in_setup(services);
    let mut probes = BTreeMap::new();
    for (probe, field) in root.at("x-stand").at("probes").entries()? {
        let probe = ProbeName(name(&field, probe)?);
        bind::unlike_in_case(&field, probe.as_str(), probes.keys().map(ProbeName::as_str))?;
        let within = required(&field, "in", "the long-running service the probe runs in")?;
        let service = service_named(services, &within)?;
        if let Some(kind) = short_lived(&services[&service]) {
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

fn short_lived(service: &Service) -> Option<&'static str> {
    match service.kind {
        ServiceKind::LongRunning => None,
        ServiceKind::Setup => Some("a setup service"),
        ServiceKind::Workload => Some("the workload"),
    }
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

/// One entry per measure: with two, which bound holds would depend on the order `report` reads
/// them in.
pub(crate) fn expect(root: &Field, policy: &Policy) -> Result<Vec<Cost>, Invalid> {
    let mut costs: Vec<Cost> = Vec::new();
    for item in root.at("x-stand").at("expect").at("cost").items()? {
        let cost = self::cost(&item, policy)?;
        if costs.iter().any(|known| known.measure == cost.measure) {
            return Err(item
                .at("measure")
                .invalid(format!("`{}` is declared twice", cost.measure)));
        }
        costs.push(cost);
    }
    Ok(costs)
}

fn cost(item: &Field, policy: &Policy) -> Result<Cost, Invalid> {
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

fn target(field: &Field, services: &BTreeMap<ServiceName, Service>) -> Result<Target, Invalid> {
    let text = field.string()?;
    let Some((service, port)) = text.rsplit_once(':') else {
        return Err(field.invalid(format!("`{text}` is not `<service>:<port>`")));
    };
    let name = ServiceName(service.to_owned());
    let Some(definition) = services.get(&name) else {
        return Err(field.invalid(format!("no service `{service}`")));
    };
    if let Some(kind) = short_lived(definition) {
        return Err(field.invalid(format!(
            "`{service}` is {kind}: a fault cuts a long-running service, which is up while the \
             workload runs"
        )));
    }
    let port = bind::port(port).map_err(|message| field.invalid(message))?;
    if !definition.expose.contains(&port) {
        return Err(field.invalid(format!(
            "`{service}` does not expose `{}`: the relay reaches a service by its `expose`",
            port.get()
        )));
    }
    Ok(Target {
        service: name,
        port,
    })
}

pub(crate) fn second_pass(
    root: &Field,
    probes: &BTreeMap<ProbeName, Probe>,
) -> Result<Option<SecondPass>, Invalid> {
    let x_stand = root.at("x-stand");
    let on = optional(&x_stand, "second_pass", Field::bool)?.unwrap_or(false);
    match (on, x_stand.at("expect").get("unchanged")) {
        (false, None) => Ok(None),
        (false, Some(field)) => Err(field.invalid(
            "`unchanged` needs `x-stand.second_pass: true`: only the second pass runs the \
             workload twice on one state",
        )),
        (true, _) => {
            let mut files: Vec<Unchanged> = Vec::new();
            for item in x_stand.at("expect").at("unchanged").items()? {
                let file = self::unchanged(&item, probes)?;
                if files.iter().any(|known| known.file == file.file) {
                    return Err(item
                        .at("file")
                        .invalid(format!("`{}` is named twice", file.file)));
                }
                files.push(file);
            }
            Ok(Some(SecondPass { unchanged: files }))
        }
    }
}

fn unchanged(item: &Field, probes: &BTreeMap<ProbeName, Probe>) -> Result<Unchanged, Invalid> {
    let file = required(item, "file", "a file of /stand/out or `probe.<name>.jsonl`")?;
    Ok(Unchanged {
        file: file.parse(|text| out_file(text, probes))?,
        reason: reason(item)?,
    })
}

/// A path that `report` joins to the `/stand/out` folder of a pass on the host, so it stays in
/// that folder on every OS, and without `.` or empty parts one file has one name; `report` reads
/// only `*.jsonl` there, and `probe.<name>.jsonl` is there after a pass only from an
/// `after-workload` probe.
fn out_file(text: &str, probes: &BTreeMap<ProbeName, Probe>) -> Result<String, String> {
    let such = "name the file by its path in /stand/out, such as `orders.jsonl`";
    if text.contains('\\') {
        return Err(format!(
            "`{text}` has a `\\`: write `/` between folders; {such}"
        ));
    }
    if text.contains(':') {
        return Err(format!(
            "`{text}` has a `:`, which a Windows host reads as a drive or a stream; {such}"
        ));
    }
    if text.starts_with('/') {
        return Err(format!("`{text}` is absolute: {such}"));
    }
    let odd = text
        .split('/')
        .find(|part| matches!(*part, "" | "." | ".."));
    match odd {
        Some("") => return Err(format!("`{text}` has an empty part: {such}")),
        Some(part) => return Err(format!("`{text}` has `{part}`: {such}")),
        None => {}
    }
    if !text.ends_with(".jsonl") {
        return Err(format!(
            "`{text}` is not a `*.jsonl` file, and `report` compares only those of /stand/out"
        ));
    }
    let Some(name) = text
        .strip_prefix("probe.")
        .and_then(|rest| rest.strip_suffix(".jsonl"))
    else {
        return Ok(text.to_owned());
    };
    let probe = probes.get(&ProbeName(name.to_owned()));
    let runs = match probe.map(|probe| probe.at) {
        None => {
            return Err(format!(
                "`{text}` is the output of a probe `{name}`, and `x-stand.probes` has none"
            ));
        }
        Some(Tap::AfterWorkload) => return Ok(text.to_owned()),
        Some(Tap::AfterSetup) => "after-setup",
        Some(Tap::OnFailure) => "on-failure",
    };
    Err(format!(
        "`{text}` is the output of the probe `{name}`, which runs {runs}: after a pass only \
         `after-workload` probes write theirs"
    ))
}
