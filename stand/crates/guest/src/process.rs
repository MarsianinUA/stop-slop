//! `run`, `stop` and `exec`. `run` starts the service's command in a process group of its own and
//! records it in the status file; `stop`, a later exec, signals that command as Docker signals a
//! container's main process, and kills its group at the grace. `exec` runs a probe or an `inspect`
//! command, dropped from root to the image's user under the probe group, until its deadline, and
//! records how it ended in `<out>.status`. When a command ends, the rest of its group is killed, as
//! a container's processes end with its main one.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command as Process, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use guest::{
    ExecArgs, ExecEnd, ExecStatus, PROBE_GID, RunArgs, StopArgs, StopReport, signal_number,
};
use serde::{Deserialize, Serialize};

const POLL: Duration = Duration::from_millis(5);
/// How long `stop` waits for `run` to record the code once the command is gone or killed.
const CODE_WAIT: Duration = Duration::from_secs(2);

/// The status file. `run` writes the pid and the group when the command starts, and its
/// `shell_code` and the time when it ends.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    pid: libc::pid_t,
    pgid: libc::pid_t,
    code: Option<i32>,
    /// Milliseconds since the Unix epoch.
    exited_at: Option<u64>,
}

pub fn run(args: &RunArgs) -> ExitCode {
    const WHO: &str = "stand-guest run";
    let mut child = match command(&args.command).process_group(0).spawn() {
        Ok(child) => child,
        Err(error) => return not_started(WHO, &args.command[0], &error),
    };
    let pid = child.id() as libc::pid_t;
    let mut status = Status {
        pid,
        pgid: pid,
        code: None,
        exited_at: None,
    };
    if let Err(error) = write_status(&args.status, &status) {
        // `stop` could not reach a command that has no status file.
        kill_group(pid);
        let _ = child.wait();
        return crate::failed(WHO, format!("write {}: {error}", args.status.display()));
    }
    let exit = child.wait().expect("wait for the command");
    kill_group(pid);
    let code = shell_code(exit);
    status.code = Some(code.into());
    status.exited_at = Some(now_ms());
    if let Err(error) = write_status(&args.status, &status) {
        eprintln!("{WHO}: write {}: {error}", args.status.display());
    }
    ExitCode::from(code)
}

pub fn stop(args: &StopArgs) -> ExitCode {
    const WHO: &str = "stand-guest stop";
    let Some(signal) = signal_number(&args.signal) else {
        return crate::failed(WHO, format!("`{}` is not a signal", args.signal));
    };
    let status = match read_status(&args.status) {
        Ok(status) => status,
        Err(message) => return crate::failed(WHO, message),
    };
    if status.code.is_some() {
        // It ended before the stop, and its pid may be another process's by now.
        return report(StopReport {
            after_ms: 0,
            killed: false,
            code: status.code,
        });
    }
    let signaled = Instant::now();
    // `ESRCH`: it exited since the status was read, which `gone_by` sees at once.
    if let Err(error) = send(status.pid, signal)
        && error.raw_os_error() != Some(libc::ESRCH)
    {
        return crate::failed(WHO, format!("signal pid {}: {error}", status.pid));
    }
    let killed = !gone_by(status.pid, signaled + args.grace);
    let after = signaled.elapsed();
    if killed {
        kill_group(status.pgid);
    }
    report(StopReport {
        after_ms: after.as_millis() as u64,
        killed,
        code: recorded_code(&args.status, Instant::now() + CODE_WAIT),
    })
}

pub fn exec(args: &ExecArgs) -> ExitCode {
    const WHO: &str = "stand-guest exec";
    let status = ExecStatus::path(&args.out);
    // An earlier exec's status would pass for this one's if this one failed before writing its own.
    if let Err(error) = remove_if_there(&status) {
        return crate::failed(WHO, format!("remove {}: {error}", status.display()));
    }
    let uid = match user_id(&args.user) {
        Ok(uid) => uid,
        Err(message) => return crate::failed(WHO, message),
    };
    let (out, err) = match outputs(&args.out) {
        Ok(files) => files,
        Err(message) => return crate::failed(WHO, message),
    };
    // Why the command did not start goes where its stderr would have gone.
    let mut start_err = match err.try_clone() {
        Ok(file) => file,
        Err(error) => return crate::failed(WHO, format!("copy the stderr handle: {error}")),
    };
    let mut probe = command(&args.argv);
    probe
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    // Without root, as in the host tests, the command runs as the caller.
    if unsafe { libc::geteuid() } == 0 {
        // SAFETY: `drop_to` makes only async-signal-safe calls, as a child before exec must.
        unsafe { probe.pre_exec(move || drop_to(uid)) };
    }
    let ended = match probe.spawn() {
        Ok(mut child) => {
            let pid = child.id() as libc::pid_t;
            let exited = poll(Instant::now() + args.deadline, || {
                child.try_wait().transpose()
            });
            let past_deadline = exited.is_none();
            kill_group(pid);
            let end = match exited.unwrap_or_else(|| child.wait()) {
                Ok(exit) => end_of(exit),
                Err(error) => return crate::failed(WHO, format!("wait for the command: {error}")),
            };
            ExecStatus {
                end,
                // Not a command that exited on its own between the last look and the kill.
                timed_out: past_deadline && end == ExecEnd::Signal(libc::SIGKILL),
            }
        }
        Err(error) => {
            let (why, code) = start_failure(&args.argv[0], &error);
            let Some(code) = code else {
                return crate::failed(WHO, why);
            };
            // The code alone still says it did not start.
            let _ = writeln!(start_err, "{WHO}: {why}");
            ExecStatus {
                end: ExecEnd::Code(code.into()),
                timed_out: false,
            }
        }
    };
    match ended.write(&status) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => crate::failed(WHO, format!("write {}: {error}", status.display())),
    }
}

/// `setgid` before `setuid`: a user other than root can no longer change its group after it. Root
/// keeps its capabilities through `setuid(0)`, so a command under root can still leave `PROBE_GID`.
fn drop_to(uid: libc::uid_t) -> io::Result<()> {
    let dropped = unsafe {
        libc::setgroups(0, std::ptr::null()) == 0
            && libc::setgid(PROBE_GID) == 0
            && libc::setuid(uid) == 0
    };
    if dropped {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The uid of the image's `USER`, which is a name or a uid, either with a `:group` that the probe
/// group replaces. A name is looked up in the guest's `/etc/passwd`; an empty `USER` is root.
fn user_id(user: &str) -> Result<libc::uid_t, String> {
    let name = user.split_once(':').map_or(user, |(name, _)| name);
    if name.is_empty() {
        return Ok(0);
    }
    if let Ok(uid) = name.parse() {
        return Ok(uid);
    }
    let passwd =
        fs::read_to_string("/etc/passwd").map_err(|error| format!("read /etc/passwd: {error}"))?;
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields[0] == name {
            return fields
                .get(2)
                .and_then(|uid| uid.parse().ok())
                .ok_or_else(|| format!("no uid for `{name}` in /etc/passwd"));
        }
    }
    Err(format!("no user `{name}` in /etc/passwd"))
}

/// Made anew: root makes them in a folder the service's processes can write, and must not follow
/// a link put at either name.
fn outputs(out: &Path) -> Result<(File, File), String> {
    let mut err = out.as_os_str().to_owned();
    err.push(".err");
    let create = |path: &Path| {
        remove_if_there(path)
            .and_then(|()| File::create_new(path))
            .map_err(|error| format!("create {}: {error}", path.display()))
    };
    Ok((create(out)?, create(Path::new(&err))?))
}

fn remove_if_there(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn command(argv: &[String]) -> Process {
    let mut command = Process::new(&argv[0]);
    command.args(&argv[1..]);
    command
}

fn not_started(who: &str, program: &str, error: &io::Error) -> ExitCode {
    let (why, code) = start_failure(program, error);
    match code {
        Some(code) => {
            eprintln!("{who}: {why}");
            ExitCode::from(code)
        }
        None => crate::failed(who, why),
    }
}

/// Why `program` did not start, and the code a shell gives it then: 127 when it is not there, 126
/// when it is no program the user may run. `None` when `stand-guest` failed itself: the fork, or the
/// drop of privileges in the child, which fail with none of these errors.
fn start_failure(program: &str, error: &io::Error) -> (String, Option<u8>) {
    let code = match error.raw_os_error() {
        Some(libc::ENOENT | libc::ENOTDIR) => Some(127),
        Some(
            libc::EACCES
            | libc::ENOEXEC
            | libc::EISDIR
            | libc::ELOOP
            | libc::ENAMETOOLONG
            | libc::ETXTBSY,
        ) => Some(126),
        _ => None,
    };
    (format!("start `{program}`: {error}"), code)
}

/// The code a shell shows: 128 + n after a death by signal n.
fn shell_code(exit: ExitStatus) -> u8 {
    match end_of(exit) {
        ExecEnd::Code(code) => code as u8,
        ExecEnd::Signal(signal) => 128 + signal as u8,
    }
}

/// `signal()` is set whenever `code()` is not: a reaped command is never only stopped.
fn end_of(exit: ExitStatus) -> ExecEnd {
    match exit.code() {
        Some(code) => ExecEnd::Code(code),
        None => ExecEnd::Signal(exit.signal().unwrap_or(0)),
    }
}

fn send(pid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The result is ignored: the group may be empty by now.
fn kill_group(pgid: libc::pid_t) {
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
}

/// `run` reaps the command as soon as it exits, so a pid with no process means it exited.
fn gone_by(pid: libc::pid_t, deadline: Instant) -> bool {
    poll(deadline, || {
        send(pid, 0)
            .is_err_and(|error| error.raw_os_error() == Some(libc::ESRCH))
            .then_some(())
    })
    .is_some()
}

/// `None` when `run` records no code by `deadline`: the command outlived the kill, or `run` is gone.
fn recorded_code(path: &Path, deadline: Instant) -> Option<i32> {
    poll(deadline, || read_status(path).ok()?.code)
}

fn poll<T>(deadline: Instant, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        thread::sleep(POLL.min(deadline - now));
    }
}

fn report(report: StopReport) -> ExitCode {
    println!(
        "{}",
        serde_json::to_string(&report).expect("a stop report serializes")
    );
    ExitCode::SUCCESS
}

/// Through a temporary file, so `stop` never reads a part.
fn write_status(path: &Path, status: &Status) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let text = serde_json::to_string(status).expect("a status serializes");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

fn read_status(path: &Path) -> Result<Status, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let status: Status = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    // The service's user can write the file, and `kill` and `killpg` read 0, -1 and 1 as whole
    // sets of processes, which a root `stop` would reach.
    if status.pid <= 1 || status.pgid <= 1 {
        return Err(format!(
            "{}: pid {} in group {} is not a command's",
            path.display(),
            status.pid,
            status.pgid
        ));
    }
    Ok(status)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}
