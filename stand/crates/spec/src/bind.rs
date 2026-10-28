//! The services and the top-level `x-stand` values as types, with the defaults of Compose and of
//! the stand. What needs every service at once is `graph`'s.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroU32;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};

use crate::tree::{Field, interpolate};
use crate::{
    Condition, Exposed, Healthcheck, Invalid, Mount, Outbound, Profiler, Repeat, Replicas, Service,
    ServiceKind, ServiceName, TREE, Timeout, units,
};

const REPEAT: NonZeroU32 = NonZeroU32::new(3).unwrap();
const SETUP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const TIMEOUT: Duration = Duration::from_secs(5 * 60);
const STOP_SIGNAL: &str = "SIGTERM";
const STOP_GRACE: Duration = Duration::from_secs(10);
/// Compose's defaults of a healthcheck; `start_period` is 0.
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_RETRIES: u32 = 3;

/// The guest folders the stand owns: its own files under `/stand`, the version disk at `/w`.
const RESERVED: [&str; 2] = ["/stand", "/w"];

/// Linux's, without the real-time ones.
const SIGNALS: &str = "SIGHUP SIGINT SIGQUIT SIGILL SIGTRAP SIGABRT SIGIOT SIGBUS SIGFPE SIGKILL \
    SIGUSR1 SIGSEGV SIGUSR2 SIGPIPE SIGALRM SIGTERM SIGSTKFLT SIGCHLD SIGCONT SIGSTOP SIGTSTP \
    SIGTTIN SIGTTOU SIGURG SIGXCPU SIGXFSZ SIGVTALRM SIGPROF SIGWINCH SIGIO SIGPOLL SIGPWR SIGSYS";

pub(crate) struct Bound {
    pub(crate) services: BTreeMap<ServiceName, Service>,
    pub(crate) workload: ServiceName,
    pub(crate) repeat: Repeat,
    pub(crate) setup_timeout: Timeout,
    pub(crate) timeout: Timeout,
}

/// `dir` is the folder of the stand file, which relative sources start from.
pub(crate) fn stand(root: &Field, dir: &Utf8Path) -> Result<Bound, Invalid> {
    let volumes = volumes(&root.at("volumes"))?;
    let services_field = root.at("services");
    let mut services = BTreeMap::new();
    let mut owners: BTreeMap<String, ServiceName> = BTreeMap::new();
    for (name, field) in services_field.entries()? {
        let name = ServiceName(self::name(&field, name)?);
        unlike_in_case(
            &field,
            name.as_str(),
            services.keys().map(ServiceName::as_str),
        )?;
        let service = service(&field, dir, &volumes)?;
        mount_owners(&field, &name, &service, &mut owners)?;
        services.insert(name, service);
    }
    if services.is_empty() {
        return Err(services_field
            .invalid("no services: the stand file is a mapping with `services` and `x-stand`"));
    }
    let x_stand = root.at("x-stand");
    let workload = required(&x_stand, "workload", "the service whose exit ends a repeat")?;
    let workload = service_named(&services, &workload)?;
    let repeat = optional(&x_stand, "repeat", |field| field.parse(units::count))?;
    let setup_timeout = optional(&x_stand, "setup_timeout", |field| {
        field.parse(units::period)
    })?;
    let timeout = optional(&x_stand, "timeout", |field| field.parse(units::period))?;
    Ok(Bound {
        services,
        workload,
        repeat: Repeat(repeat.unwrap_or(REPEAT)),
        setup_timeout: Timeout(setup_timeout.unwrap_or(SETUP_TIMEOUT)),
        timeout: Timeout(timeout.unwrap_or(TIMEOUT)),
    })
}

/// A name of a service or a probe.
pub(crate) fn name(field: &Field, name: &str) -> Result<String, Invalid> {
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
    let message = if name.contains('.') {
        format!(
            "`{name}` has a `.`, which would end the subject of a measure name such as \
             `api.wall`: write `-` or `_`"
        )
    } else if !name
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !name.bytes().all(word)
    {
        format!("`{name}` is not a name: letters, digits, `_` and `-`, from a letter or digit")
    } else if name == "stand" || name == "volume" {
        format!(
            "`{name}` is reserved for measures of the stand and of volumes, such as `stand.wall` \
             and `volume:pgdata.bytes`"
        )
    } else if is_device(name) {
        not_on_windows(name)
    } else {
        return Ok(name.to_owned());
    };
    Err(field.invalid(message))
}

pub(crate) fn unlike_in_case<'a>(
    field: &Field,
    name: &str,
    mut others: impl Iterator<Item = &'a str>,
) -> Result<(), Invalid> {
    match others.find(|other| other.eq_ignore_ascii_case(name)) {
        Some(other) => Err(field.invalid(format!(
            "`{name}` differs from `{other}` only in case, and the stand names files on the host \
             after both: Windows and macOS would give them one"
        ))),
        None => Ok(()),
    }
}

/// Windows opens a device for these names, also with an extension (`nul.data`).
fn is_device(name: &str) -> bool {
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_ascii_lowercase();
    let port = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|digit| matches!(digit.as_bytes(), [digit] if digit.is_ascii_digit()))
    };
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul") || port("com") || port("lpt")
}

fn not_on_windows(name: &str) -> String {
    format!("`{name}` is a device name on Windows, and the stand names files on the host after it")
}

pub(crate) fn service_named(
    services: &BTreeMap<ServiceName, Service>,
    field: &Field,
) -> Result<ServiceName, Invalid> {
    let name = ServiceName(field.string()?);
    if services.contains_key(&name) {
        Ok(name)
    } else {
        Err(field.invalid(format!("no service `{}`", name.as_str())))
    }
}

pub(crate) fn optional<'a, T>(
    field: &Field<'a>,
    key: &str,
    read: impl FnOnce(&Field<'a>) -> Result<T, Invalid>,
) -> Result<Option<T>, Invalid> {
    field.get(key).map(|value| read(&value)).transpose()
}

/// The value of `key`, which `why` needs.
pub(crate) fn required<'a>(field: &Field<'a>, key: &str, why: &str) -> Result<Field<'a>, Invalid> {
    field
        .get(key)
        .ok_or_else(|| field.at(key).invalid(format!("required: {why}")))
}

/// The names of the top-level `volumes`, whose values `allow` keeps empty.
fn volumes(field: &Field) -> Result<BTreeSet<String>, Invalid> {
    let mut names = BTreeSet::new();
    for (name, value) in field.entries()? {
        if !is_volume_name(name) {
            return Err(value.invalid(not_volume_name(name)));
        }
        if is_device(name) {
            return Err(value.invalid(not_on_windows(name)));
        }
        unlike_in_case(&value, name, names.iter().map(String::as_str))?;
        names.insert(name.to_owned());
    }
    Ok(names)
}

/// The characters of Docker's volume names, `[a-zA-Z0-9][a-zA-Z0-9_.-]*`, without its two-char
/// minimum: the name goes into the name of its disk on the host, which a `/` would lead out of the
/// folder of disks.
fn is_volume_name(name: &str) -> bool {
    let word = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-');
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && name.bytes().all(word)
}

fn not_volume_name(name: &str) -> String {
    format!(
        "`{name}` is not a volume name: letters, digits, `_`, `.` and `-`, from a letter or digit"
    )
}

fn service(field: &Field, dir: &Utf8Path, volumes: &BTreeSet<String>) -> Result<Service, Invalid> {
    let image = required(
        field,
        "image",
        "the stand starts every service from its image",
    )?;
    let deploy = field.at("deploy");
    let limits = deploy.at("resources").at("limits");
    let why = "the stand gives every service its own limits and budgets the host by them";
    let x_stand = field.at("x-stand");
    let mounts = optional(field, "volumes", |field| mounts(field, dir, volumes))?;
    Ok(Service {
        kind: ServiceKind::LongRunning,
        image: image.string()?,
        entrypoint: optional(field, "entrypoint", argv)?,
        command: optional(field, "command", argv)?,
        environment: optional(field, "environment", environment)?.unwrap_or_default(),
        working_dir: optional(field, "working_dir", Field::string)?,
        mounts: mounts.unwrap_or_default(),
        expose: optional(field, "expose", expose)?.unwrap_or_default(),
        depends_on: optional(field, "depends_on", depends_on)?.unwrap_or_default(),
        healthcheck: optional(field, "healthcheck", healthcheck)?,
        stop_signal: optional(field, "stop_signal", |field| field.parse(signal))?
            .unwrap_or_else(|| STOP_SIGNAL.to_owned()),
        stop_grace: optional(field, "stop_grace_period", |field| {
            field.parse(units::duration)
        })?
        .unwrap_or(STOP_GRACE),
        cpus: required(&limits, "cpus", why)?.parse(units::cpus)?,
        memory: required(&limits, "memory", why)?.parse(units::bytes)?,
        replicas: Replicas(
            optional(&deploy, "replicas", |field| field.parse(units::count))?
                .unwrap_or(NonZeroU32::MIN),
        ),
        profile: optional(&x_stand, "profile", |field| field.parse(profiler))?,
        outbound: optional(&x_stand, "network", |field| field.parse(outbound))?
            .unwrap_or(Outbound::Off),
    })
}

fn mount_owners(
    field: &Field,
    name: &ServiceName,
    service: &Service,
    owners: &mut BTreeMap<String, ServiceName>,
) -> Result<(), Invalid> {
    for (index, mount) in service.mounts.iter().enumerate() {
        let Mount::Volume { name: volume, .. } = mount else {
            continue;
        };
        let at = field.at("volumes").item(index);
        if service.on_tree() {
            return Err(at.invalid(format!(
                "`{volume}` on a service with `${{STAND_REPO}}`: such a service starts from the \
                 snapshot of the version, which cannot add a disk; mount the volume in another \
                 service"
            )));
        }
        if let Some(owner) = owners.insert(volume.clone(), name.clone()) {
            let by = if owner == *name {
                "twice".to_owned()
            } else {
                format!("by `{}` too", owner.as_str())
            };
            return Err(at.invalid(format!(
                "`{volume}` is mounted {by}: a volume is the disk of one service, mounted once"
            )));
        }
        if service.replicas.get() > 1 {
            return Err(field.at("deploy").at("replicas").invalid(format!(
                "a service with the volume `{volume}` has one replica: the volume is the disk of \
                 one VM"
            )));
        }
    }
    Ok(())
}

/// `command` and `entrypoint`: a list, or a string split as a shell would, as Compose does.
fn argv(field: &Field) -> Result<Vec<String>, Invalid> {
    if field.is_list() {
        return field.strings();
    }
    let text = field.string()?;
    shlex::split(&text).ok_or_else(|| {
        field.invalid(format!(
            "`{text}` does not split into words: close the quote"
        ))
    })
}

/// A variable without a value would come from the host in Compose; the stand takes nothing from
/// the host, so both versions get the same environment.
fn environment(field: &Field) -> Result<BTreeMap<String, String>, Invalid> {
    let unset =
        |name: &str| format!("`{name}` has no value, and the stand takes none from the host");
    let mut environment = BTreeMap::new();
    if field.is_list() {
        for item in field.items()? {
            let text = item.string()?;
            let Some((name, value)) = text.split_once('=') else {
                return Err(item.invalid(unset(&text)));
            };
            if environment
                .insert(name.to_owned(), value.to_owned())
                .is_some()
            {
                return Err(item.invalid(format!("`{name}` is set twice")));
            }
        }
        return Ok(environment);
    }
    for (name, value) in field.entries()? {
        if value.is_null() {
            return Err(value.invalid(unset(name)));
        }
        environment.insert(name.to_owned(), value.string()?);
    }
    Ok(environment)
}

fn mounts(
    field: &Field,
    dir: &Utf8Path,
    volumes: &BTreeSet<String>,
) -> Result<Vec<Mount>, Invalid> {
    let mut targets = BTreeSet::new();
    let mut mounts = Vec::new();
    for item in field.items()? {
        let mount = mount(&item, dir, volumes)?;
        let target = match &mount {
            Mount::Tree => TREE,
            Mount::Harness { target, .. } | Mount::Volume { target, .. } => target,
        };
        if !targets.insert(target.to_owned()) {
            return Err(item.invalid(format!("a second mount on `{target}`")));
        }
        mounts.push(mount);
    }
    Ok(mounts)
}

/// The short form only: `source:target`, `source:target:ro`. The source is `${STAND_REPO}`, a
/// folder beside the stand file (from `.`) or the name of a top-level volume.
fn mount(item: &Field, dir: &Utf8Path, volumes: &BTreeSet<String>) -> Result<Mount, Invalid> {
    if item.is_map() {
        return Err(item.invalid("write the short form, `source:target` or `source:target:ro`"));
    }
    let text = item.raw()?;
    // `C:\data:/data` would split into three parts.
    if is_drive(text) {
        return Err(item.invalid(absolute(text)));
    }
    let (source, target, mode) = match text.split(':').collect::<Vec<_>>()[..] {
        [source, target] => (source, target, None),
        [source, target, mode] => (source, target, Some(mode)),
        _ => {
            return Err(item.invalid(format!(
                "`{text}` is not `source:target` or `source:target:ro`"
            )));
        }
    };
    let read_only = match mode {
        None | Some("rw") => false,
        Some("ro") => true,
        Some(mode) => {
            return Err(item.invalid(format!("`{mode}` is not a mode: write `ro` or none")));
        }
    };
    if source == "${STAND_REPO}" {
        return tree(item, target, read_only);
    }
    if source.starts_with("${STAND_REPO}/") {
        return Err(item.invalid(format!(
            "`{source}`: `${{STAND_REPO}}` mounts whole, on {TREE}; for a subfolder set \
             `working_dir`"
        )));
    }
    let source = interpolate(source).map_err(|message| item.invalid(message))?;
    if source.starts_with(['/', '\\', '~']) {
        return Err(item.invalid(absolute(&source)));
    }
    let target = interpolate(target).map_err(|message| item.invalid(message))?;
    let target = guest_path(&target)
        .ok_or_else(|| item.invalid(format!("`{target}` is not a path from `/`")))?;
    if target == "/" {
        return Err(item.invalid("a mount on `/` would cover /stand and /w, which the stand keeps"));
    }
    if let Some(reserved) = RESERVED.iter().find(|reserved| is_under(&target, reserved)) {
        return Err(item.invalid(format!(
            "`{target}` is under {reserved}, which the stand keeps: its own files are under \
             /stand, and only `${{STAND_REPO}}:{TREE}` goes under /w"
        )));
    }
    if source.starts_with('.') {
        let leaves = |how: String| {
            item.invalid(format!(
                "`{source}` leaves the folder of the stand file{how}, and the seal of the recipe \
                 keeps only that folder"
            ))
        };
        let path = harness(dir, &source).ok_or_else(|| leaves(String::new()))?;
        if let Some(link) = link_on(dir, &path) {
            return Err(leaves(format!(" through the link `{link}`")));
        }
        if !path.exists() {
            return Err(item.invalid(format!(
                "`{source}` is not in the folder of the stand file: the sandbox would make it \
                 there, and the recipe would change after its seal"
            )));
        }
        return Ok(Mount::Harness {
            source: path,
            target,
            read_only,
        });
    }
    if !is_volume_name(&source) {
        return Err(item.invalid(format!(
            "{}; a folder beside the stand file starts with `./`",
            not_volume_name(&source)
        )));
    }
    if !volumes.contains(&source) {
        return Err(item.invalid(format!("no volume `{source}` in the top-level `volumes`")));
    }
    Ok(Mount::Volume {
        name: source,
        target,
        read_only,
    })
}

/// Each service gets its own version disk with the tree at `TREE`: nothing else is mounted from
/// it, and it is never read-only.
fn tree(item: &Field, target: &str, read_only: bool) -> Result<Mount, Invalid> {
    if guest_path(target).as_deref() != Some(TREE) {
        return Err(item.invalid(format!(
            "`${{STAND_REPO}}` mounts on {TREE}, not `{target}`: the version disk is at /w and \
             its tree at {TREE}"
        )));
    }
    if read_only {
        return Err(item.invalid(
            "`${STAND_REPO}` is not read-only: each service writes its own version disk",
        ));
    }
    Ok(Mount::Tree)
}

/// `C:\data` or `C:/data`.
fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn absolute(source: &str) -> String {
    format!(
        "`{source}` is a host path; a bind takes a folder beside the stand file, such as `.` or \
         `./harness`, or `${{STAND_REPO}}`"
    )
}

/// `source` from `dir` with `.` and `..` folded, or `None` when it leaves `dir`.
fn harness(dir: &Utf8Path, source: &str) -> Option<Utf8PathBuf> {
    let mut parts = Vec::new();
    for part in source.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(
        parts
            .iter()
            .fold(dir.to_owned(), |path, part| path.join(part)),
    )
}

/// The first link on the way from `dir` to `path`, as a path from `dir`: a bind follows it, while
/// the recipe and its seal keep only the link.
fn link_on(dir: &Utf8Path, path: &Utf8Path) -> Option<Utf8PathBuf> {
    let mut on = dir.to_owned();
    for part in path.strip_prefix(dir).ok()?.components() {
        on.push(part);
        if fs::symlink_metadata(&on).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return on.strip_prefix(dir).ok().map(Utf8Path::to_owned);
        }
    }
    None
}

/// A path of the guest from `/` with `.`, `..` and empty parts folded; `None` when relative.
fn guest_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.strip_prefix('/')?.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

fn is_under(path: &str, folder: &str) -> bool {
    path.strip_prefix(folder)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn expose(field: &Field) -> Result<Vec<Exposed>, Invalid> {
    let mut ports = Vec::new();
    for item in field.items()? {
        let port = item.parse(port)?;
        if ports.contains(&port) {
            return Err(item.invalid(format!("{} is listed twice", port.get())));
        }
        ports.push(port);
    }
    Ok(ports)
}

/// `5432` or `"5432/tcp"`: the relay carries TCP only.
pub(crate) fn port(text: &str) -> Result<Exposed, String> {
    let number = text.strip_suffix("/tcp").unwrap_or(text);
    match number.parse::<u16>() {
        Ok(port) if port > 0 && units::is_whole(number) => Ok(Exposed(port)),
        _ if text.ends_with("/udp") => Err(format!("`{text}`: the stand relays TCP only")),
        _ => Err(format!("`{text}` is not a port from 1 to 65535")),
    }
}

/// A list means `service_started`, as does a long entry without `condition`.
fn depends_on(field: &Field) -> Result<BTreeMap<ServiceName, Condition>, Invalid> {
    if field.is_list() {
        return Ok(field
            .strings()?
            .into_iter()
            .map(|name| (ServiceName(name), Condition::Started))
            .collect());
    }
    let mut depends_on = BTreeMap::new();
    for (name, value) in field.entries()? {
        let condition = optional(&value, "condition", |field| field.parse(condition))?;
        depends_on.insert(
            ServiceName(name.to_owned()),
            condition.unwrap_or(Condition::Started),
        );
    }
    Ok(depends_on)
}

const CONDITIONS: [Condition; 3] = [
    Condition::Started,
    Condition::Healthy,
    Condition::CompletedSuccessfully,
];

fn condition(text: &str) -> Result<Condition, String> {
    CONDITIONS
        .into_iter()
        .find(|&condition| condition_name(condition) == text)
        .ok_or_else(|| {
            format!(
                "`{text}` is not `service_started`, `service_healthy` or \
                 `service_completed_successfully`"
            )
        })
}

pub(crate) fn condition_name(condition: Condition) -> &'static str {
    match condition {
        Condition::Started => "service_started",
        Condition::Healthy => "service_healthy",
        Condition::CompletedSuccessfully => "service_completed_successfully",
    }
}

fn healthcheck(field: &Field) -> Result<Healthcheck, Invalid> {
    let test = required(field, "test", "the stand runs no healthcheck of the image")?;
    let retries = optional(field, "retries", |field| field.parse(units::count))?;
    Ok(Healthcheck {
        test: health_test(&test)?,
        interval: optional(field, "interval", |field| field.parse(units::period))?
            .unwrap_or(HEALTH_INTERVAL),
        timeout: optional(field, "timeout", |field| field.parse(units::period))?
            .unwrap_or(HEALTH_TIMEOUT),
        retries: retries.map_or(HEALTH_RETRIES, NonZeroU32::get),
        start_period: optional(field, "start_period", |field| field.parse(units::duration))?
            .unwrap_or_default(),
    })
}

fn health_test(field: &Field) -> Result<Vec<String>, Invalid> {
    let shell = |text: String| vec!["sh".to_owned(), "-c".to_owned(), text];
    if !field.is_list() {
        return Ok(shell(field.string()?));
    }
    match &field.strings()?[..] {
        [first, rest @ ..] if first == "CMD" && !rest.is_empty() => Ok(rest.to_vec()),
        [first, text] if first == "CMD-SHELL" => Ok(shell(text.clone())),
        _ => Err(field.invalid(
            "write `[\"CMD\", <argv>...]`, `[\"CMD-SHELL\", <command>]` or a command string",
        )),
    }
}

fn signal(text: &str) -> Result<String, String> {
    if SIGNALS.split_whitespace().any(|signal| signal == text) {
        Ok(text.to_owned())
    } else {
        Err(format!(
            "`{text}` is not a Linux signal such as `SIGTERM` or `SIGINT`"
        ))
    }
}

fn profiler(text: &str) -> Result<Profiler, String> {
    match text {
        "node" => Ok(Profiler::Node),
        _ => Err(format!("`{text}` is not a profiler: write `node`")),
    }
}

fn outbound(text: &str) -> Result<Outbound, String> {
    match text {
        "public" => Ok(Outbound::Public),
        _ => Err(format!(
            "`{text}`: write `public` to let the service out; without the key it stays in"
        )),
    }
}
