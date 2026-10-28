//! What the sandbox adapter and `stand-guest` exchange: the subcommand arguments, the files of
//! `/stand/net`, the status of `run` and of an `exec`. The adapter writes the arguments and
//! `FaultSwitch` and reads `Counters`, `RunStatus`, `StopReport` and `ExecStatus`; the binary does the
//! opposite.

use std::fs;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The group of probe processes: the netfilter rules of `net` match it, `exec` takes it. Outside `guest`
/// only the adapter may read it; `run`, `Launch` and the subcommand arguments never carry it.
pub const PROBE_GID: u32 = 65533;

/// The arguments of `stand-guest`, one variant per subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Net(Routes),
    Run(RunArgs),
    Stop(StopArgs),
    Exec(ExecArgs),
    Trim(TrimArgs),
}

/// `stand-guest net`: a separate exec as root, before the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routes {
    /// `host.microsandbox.internal` in a guest, `127.0.0.1` in tests.
    pub upstream_host: String,
    pub entry: Entry,
    pub routes: Vec<Route>,
    pub unexposed: Vec<UnexposedName>,
    pub counters: PathBuf,
    /// Watched for a `FaultSwitch`.
    pub fault: PathBuf,
}

/// `Redirect`: one listener on `127.0.0.1:<free port>`, a REDIRECT rule to it, the target by `SO_ORIGINAL_DST`;
/// also the rules of `PROBE_GID`. `Direct`: a listener on each `address:port`, for tests on the host;
/// unexposed names are counted only under `Redirect`, since nothing tells `Direct` their ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Redirect,
    Direct,
}

/// `name:port` at `address` goes round-robin over `upstream`, one host port per replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub port: u16,
    pub address: Ipv4Addr,
    pub upstream: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexposedName {
    pub name: String,
    pub address: Ipv4Addr,
}

/// `stand-guest run`: runs the command in its own process group, as the user of the exec, its stdout and
/// stderr through pipes of that user, which it passes on; exits with its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub status: PathBuf,
    pub command: Vec<String>,
}

/// `stand-guest stop`: the signal to the command's process in `status` (as Docker signals the main process),
/// the wait, a `SIGKILL` to its group at `grace`; prints a `StopReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopArgs {
    pub status: PathBuf,
    pub signal: String,
    pub grace: Duration,
}

/// `stand-guest exec`: starts as root, drops the supplementary groups, `setgid(PROBE_GID)`, then `setuid(user)`;
/// stdout to `out`, stderr to `<out>.err`, how the command ended to `<out>.status` (`ExecStatus`), the
/// process group killed at `deadline`. `user` is the image's `USER`; empty means root. Only a user other
/// than root is held in `PROBE_GID`: root keeps its capabilities, so a probe under root can leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecArgs {
    pub out: PathBuf,
    pub deadline: Duration,
    pub user: String,
    pub argv: Vec<String>,
}

/// `stand-guest trim`: as root, the FITRIM ioctl on each mount point of an owned disk, without image tools.
/// The root disk does not support it, so the adapter never names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimArgs {
    pub mounts: Vec<PathBuf>,
}

impl Command {
    /// The arguments after the binary's path, for example
    /// `stop --status /stand/run/status.json --signal SIGTERM --grace 10000`.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Args::default();
        match self {
            Command::Net(routes) => {
                args.word("net");
                args.pair("--upstream", &routes.upstream_host);
                args.path("--counters", &routes.counters);
                args.path("--fault", &routes.fault);
                args.word(match routes.entry {
                    Entry::Redirect => "--redirect",
                    Entry::Direct => "--direct",
                });
                for route in &routes.routes {
                    let upstream: Vec<String> = route.upstream.iter().map(u16::to_string).collect();
                    let value = format!(
                        "{}:{}@{}={}",
                        route.name,
                        route.port,
                        route.address,
                        upstream.join(",")
                    );
                    args.pair("--route", &value);
                }
                for name in &routes.unexposed {
                    args.pair("--unexposed", &format!("{}@{}", name.name, name.address));
                }
            }
            Command::Run(run) => {
                args.word("run");
                args.path("--status", &run.status);
                args.rest(&run.command);
            }
            Command::Stop(stop) => {
                args.word("stop");
                args.path("--status", &stop.status);
                args.pair("--signal", &stop.signal);
                args.pair("--grace", &stop.grace.as_millis().to_string());
            }
            Command::Exec(exec) => {
                args.word("exec");
                args.path("--out", &exec.out);
                args.pair("--deadline", &exec.deadline.as_millis().to_string());
                args.pair("--user", &exec.user);
                args.rest(&exec.argv);
            }
            Command::Trim(trim) => {
                args.word("trim");
                for mount in &trim.mounts {
                    args.path("--mount", mount);
                }
            }
        }
        args.0
    }

    pub fn parse(args: &[String]) -> Result<Command, GuestError> {
        let (sub, rest) = args
            .split_first()
            .ok_or_else(|| GuestError::args("no subcommand"))?;
        let mut flags = Flags::read(rest)?;
        let command = match sub.as_str() {
            "net" => Command::Net(Routes {
                upstream_host: flags.one("--upstream")?,
                entry: flags.entry()?,
                routes: flags
                    .many("--route")
                    .iter()
                    .map(|r| route(r))
                    .collect::<Result<_, _>>()?,
                unexposed: flags
                    .many("--unexposed")
                    .iter()
                    .map(|u| unexposed(u))
                    .collect::<Result<_, _>>()?,
                counters: flags.one("--counters")?.into(),
                fault: flags.one("--fault")?.into(),
            }),
            "run" => Command::Run(RunArgs {
                status: flags.one("--status")?.into(),
                command: flags.command()?,
            }),
            "stop" => Command::Stop(StopArgs {
                status: flags.one("--status")?.into(),
                signal: flags.one("--signal")?,
                grace: flags.millis("--grace")?,
            }),
            "exec" => Command::Exec(ExecArgs {
                out: flags.one("--out")?.into(),
                deadline: flags.millis("--deadline")?,
                user: flags.one("--user")?,
                argv: flags.command()?,
            }),
            "trim" => {
                let mounts: Vec<PathBuf> = flags
                    .many("--mount")
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                if mounts.is_empty() {
                    return Err(GuestError::args("trim needs a --mount"));
                }
                Command::Trim(TrimArgs { mounts })
            }
            other => return Err(GuestError::Args(format!("unknown subcommand `{other}`"))),
        };
        flags.done(sub)?;
        Ok(command)
    }
}

#[derive(Default)]
struct Args(Vec<String>);

impl Args {
    fn word(&mut self, word: &str) {
        self.0.push(word.to_owned());
    }

    fn pair(&mut self, flag: &str, value: &str) {
        self.word(flag);
        self.word(value);
    }

    fn path(&mut self, flag: &str, path: &Path) {
        self.pair(flag, &path.to_string_lossy());
    }

    fn rest(&mut self, command: &[String]) {
        self.word("--");
        self.0.extend(command.iter().cloned());
    }
}

/// Flags in any order, each `--flag value` except `--redirect` and `--direct`; after `--` the command.
struct Flags {
    pairs: Vec<(String, String)>,
    words: Vec<String>,
    command: Option<Vec<String>>,
}

impl Flags {
    fn read(args: &[String]) -> Result<Flags, GuestError> {
        let mut flags = Flags {
            pairs: Vec::new(),
            words: Vec::new(),
            command: None,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--" => {
                    flags.command = Some(it.cloned().collect());
                    break;
                }
                "--redirect" | "--direct" => flags.words.push(arg.clone()),
                flag if flag.starts_with("--") => {
                    let value = it
                        .next()
                        .ok_or_else(|| GuestError::Args(format!("{flag} needs a value")))?;
                    flags.pairs.push((flag.to_owned(), value.clone()));
                }
                other => return Err(GuestError::Args(format!("unexpected `{other}`"))),
            }
        }
        Ok(flags)
    }

    fn one(&mut self, flag: &str) -> Result<String, GuestError> {
        let mut values = self.many(flag);
        match values.len() {
            1 => Ok(values.remove(0)),
            0 => Err(GuestError::Args(format!("{flag} is missing"))),
            _ => Err(GuestError::Args(format!("{flag} is given more than once"))),
        }
    }

    fn many(&mut self, flag: &str) -> Vec<String> {
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pairs)
            .into_iter()
            .partition(|(f, _)| f == flag);
        self.pairs = kept;
        taken.into_iter().map(|(_, value)| value).collect()
    }

    fn millis(&mut self, flag: &str) -> Result<Duration, GuestError> {
        let text = self.one(flag)?;
        let ms = text.parse().map_err(|_| {
            GuestError::Args(format!("{flag} `{text}` is not a number of milliseconds"))
        })?;
        Ok(Duration::from_millis(ms))
    }

    fn entry(&mut self) -> Result<Entry, GuestError> {
        match std::mem::take(&mut self.words).as_slice() {
            [word] if word == "--redirect" => Ok(Entry::Redirect),
            [word] if word == "--direct" => Ok(Entry::Direct),
            _ => Err(GuestError::args("net needs one of --redirect and --direct")),
        }
    }

    fn command(&mut self) -> Result<Vec<String>, GuestError> {
        match self.command.take() {
            Some(command) if !command.is_empty() => Ok(command),
            _ => Err(GuestError::args("the command after `--` is missing")),
        }
    }

    fn done(self, sub: &str) -> Result<(), GuestError> {
        let flags = self.pairs.iter().map(|(flag, _)| flag.as_str());
        let unused: Vec<&str> = flags.chain(self.words.iter().map(String::as_str)).collect();
        if !unused.is_empty() {
            return Err(GuestError::Args(format!(
                "{sub} does not take {}",
                unused.join(", ")
            )));
        }
        if self.command.is_some() {
            return Err(GuestError::Args(format!("{sub} takes no command")));
        }
        Ok(())
    }
}

fn route(text: &str) -> Result<Route, GuestError> {
    let bad = || {
        GuestError::Args(format!(
            "--route `{text}` is not <name>:<port>@<ip>=<port>[,<port>...]"
        ))
    };
    let (target, upstream) = text.split_once('=').ok_or_else(bad)?;
    let (name_port, address) = target.split_once('@').ok_or_else(bad)?;
    let (name, port) = name_port.rsplit_once(':').ok_or_else(bad)?;
    let upstream: Vec<u16> = upstream
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    if name.is_empty() {
        return Err(bad());
    }
    Ok(Route {
        name: name.to_owned(),
        port: port.parse().map_err(|_| bad())?,
        address: address.parse().map_err(|_| bad())?,
        upstream,
    })
}

fn unexposed(text: &str) -> Result<UnexposedName, GuestError> {
    let bad = || GuestError::Args(format!("--unexposed `{text}` is not <name>@<ip>"));
    let (name, address) = text.split_once('@').ok_or_else(bad)?;
    if name.is_empty() {
        return Err(bad());
    }
    Ok(UnexposedName {
        name: name.to_owned(),
        address: address.parse().map_err(|_| bad())?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub targets: Vec<TargetCounters>,
    pub unexposed: Vec<UnexposedCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetCounters {
    /// `name:port`.
    pub target: String,
    pub connections: u64,
    pub open: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
    /// Responses by class 1xx..5xx; `None` unless a connection began with an HTTP method.
    pub http: Option<[u64; 5]>,
    pub drops: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnexposedCount {
    pub name: String,
    pub attempts: u64,
}

impl Counters {
    /// Written whole through a temporary file.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        write_whole(path, &json(self))
    }

    pub fn parse(text: &str) -> Result<Counters, GuestError> {
        serde_json::from_str(text).map_err(|error| GuestError::Counters(error.to_string()))
    }
}

/// Written by the adapter; `net` applies it from the moment it sees the file until the file is gone,
/// if it has a route for `target`. `down` resets the target's open connections and each new one;
/// `slow` holds each new connection `delay_ms` before its first byte.
/// `{"target": "db:5432", "mode": "down"}` or `{"target": "db:5432", "mode": "slow", "delayMs": 2000}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultSwitch {
    pub target: String,
    #[serde(flatten)]
    pub mode: FaultSwitchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum FaultSwitchMode {
    Down,
    Slow { delay_ms: u64 },
}

impl FaultSwitch {
    pub fn write(&self, path: &Path) -> io::Result<()> {
        write_whole(path, &json(self))
    }

    pub fn parse(text: &str) -> Result<FaultSwitch, GuestError> {
        serde_json::from_str(text)
            .map_err(|error| GuestError::File(format!("fault switch: {error}")))
    }
}

/// The status file of `stand-guest run`, written when the command starts and again with its shell code
/// when it ends. `run` exits with that code too, but its own failure is also 125, so the adapter takes
/// the code of a service only from this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    pub pid: i32,
    pub pgid: i32,
    pub code: Option<i32>,
    /// Milliseconds since the Unix epoch.
    pub exited_at: Option<u64>,
}

impl RunStatus {
    pub fn write(&self, path: &Path) -> io::Result<()> {
        write_whole(path, &json(self))
    }

    pub fn parse(text: &str) -> Result<RunStatus, GuestError> {
        serde_json::from_str(text).map_err(|error| GuestError::File(format!("run status: {error}")))
    }
}

/// One JSON line on the stdout of `stand-guest stop`. `after_ms` runs from the signal to the exit, or to the
/// kill when `killed`; `code` is `None` when `run` recorded none within 2 s of either: the command outlived
/// the kill, or `run` itself is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopReport {
    pub after_ms: u64,
    pub killed: bool,
    pub code: Option<i32>,
}

impl StopReport {
    pub fn parse(text: &str) -> Result<StopReport, GuestError> {
        serde_json::from_str(text.trim())
            .map_err(|error| GuestError::File(format!("stop report: {error}")))
    }
}

/// How the command of `stand-guest exec` ended, in `<out>.status` beside its outputs: a channel of its
/// own, since the command may exit with any code, 124 and 125 too. `stand-guest exec` exits 0 once the
/// file is written, and 125 without it when it fails itself. `timed_out`: killed at the deadline.
/// `{"code": 3, "timedOut": false}` or `{"signal": 9, "timedOut": true}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecStatus {
    #[serde(flatten)]
    pub end: ExecEnd,
    pub timed_out: bool,
}

/// A command that could not start has the code a shell gives it: 126, or 127 when it was not
/// found; the reason is in `<out>.err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecEnd {
    Code(i32),
    Signal(i32),
}

impl ExecStatus {
    /// `<out>.status`, for the `out` of an `ExecArgs`.
    pub fn path(out: &Path) -> PathBuf {
        let mut path = out.as_os_str().to_owned();
        path.push(".status");
        path.into()
    }

    pub fn write(&self, path: &Path) -> io::Result<()> {
        write_whole(path, &json(self))
    }

    pub fn parse(text: &str) -> Result<ExecStatus, GuestError> {
        serde_json::from_str(text)
            .map_err(|error| GuestError::File(format!("exec status: {error}")))
    }
}

/// The Linux number of a signal name `spec` allows; x86_64 and aarch64 share them. A table, not
/// `libc`: the guest is always Linux, and this library builds on every host. On a macOS host only
/// the tests send a signal, `SIGTERM`, which is 15 there too.
pub fn signal_number(name: &str) -> Option<i32> {
    let number = match name {
        "SIGHUP" => 1,
        "SIGINT" => 2,
        "SIGQUIT" => 3,
        "SIGILL" => 4,
        "SIGTRAP" => 5,
        "SIGABRT" | "SIGIOT" => 6,
        "SIGBUS" => 7,
        "SIGFPE" => 8,
        "SIGKILL" => 9,
        "SIGUSR1" => 10,
        "SIGSEGV" => 11,
        "SIGUSR2" => 12,
        "SIGPIPE" => 13,
        "SIGALRM" => 14,
        "SIGTERM" => 15,
        "SIGSTKFLT" => 16,
        "SIGCHLD" => 17,
        "SIGCONT" => 18,
        "SIGSTOP" => 19,
        "SIGTSTP" => 20,
        "SIGTTIN" => 21,
        "SIGTTOU" => 22,
        "SIGURG" => 23,
        "SIGXCPU" => 24,
        "SIGXFSZ" => 25,
        "SIGVTALRM" => 26,
        "SIGPROF" => 27,
        "SIGWINCH" => 28,
        "SIGIO" | "SIGPOLL" => 29,
        "SIGPWR" => 30,
        "SIGSYS" => 31,
        _ => return None,
    };
    Some(number)
}

#[derive(Debug, thiserror::Error)]
pub enum GuestError {
    #[error("bad arguments: {0}")]
    Args(String),
    #[error("bad counters: {0}")]
    Counters(String),
    #[error("bad file: {0}")]
    File(String),
}

impl GuestError {
    fn args(message: &str) -> GuestError {
        GuestError::Args(message.to_owned())
    }
}

fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("the files of guest serialize")
}

/// The reader on the other side of the bind sees the old file or the new one, never a part. The
/// temp file is made anew: in the guest root writes it in a folder the service's user can write,
/// and must not follow a link that user put at its name.
fn write_whole(path: &Path, text: &str) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    match fs::remove_file(&tmp) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::File::create_new(&tmp)?.write_all(text.as_bytes())?;
    fs::rename(&tmp, path)
}
