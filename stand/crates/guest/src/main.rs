//! `stand-guest`, run by the sandbox adapter inside each guest. Exit codes follow `timeout(1)`: the
//! command's own code, 124 when `exec` hit its deadline, 125 when `stand-guest` itself failed (one
//! line on stderr says why), 126 when the command could not start, 127 when it was not found,
//! 128 + n when signal n killed it.

#[cfg(unix)]
mod net;
#[cfg(target_os = "linux")]
mod nf;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod relay;
#[cfg(unix)]
mod sniff;
#[cfg(target_os = "linux")]
mod trim;

use std::process::ExitCode;

#[cfg(unix)]
fn main() -> ExitCode {
    let args: Result<Vec<String>, _> = std::env::args_os()
        .skip(1)
        .map(std::ffi::OsString::into_string)
        .collect();
    let Ok(args) = args else {
        return failed("stand-guest", "an argument is not UTF-8");
    };
    match guest::Command::parse(&args) {
        Ok(guest::Command::Net(routes)) => net::run(&routes),
        Ok(guest::Command::Run(args)) => process::run(&args),
        Ok(guest::Command::Stop(args)) => process::stop(&args),
        Ok(guest::Command::Exec(args)) => process::exec(&args),
        Ok(guest::Command::Trim(args)) => run_trim(&args),
        Err(error) => failed("stand-guest", error),
    }
}

#[cfg(target_os = "linux")]
fn run_trim(args: &guest::TrimArgs) -> ExitCode {
    trim::run(args)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn run_trim(_: &guest::TrimArgs) -> ExitCode {
    failed("stand-guest trim", "FITRIM is an ioctl only Linux has")
}

#[cfg(unix)]
fn failed(who: &str, why: impl std::fmt::Display) -> ExitCode {
    eprintln!("{who}: {why}");
    ExitCode::from(125)
}

/// The guest is always Linux; on Windows only the library is real, so the workspace still builds there.
#[cfg(not(unix))]
fn main() -> ExitCode {
    eprintln!("stand-guest runs only inside a Linux guest");
    ExitCode::from(125)
}
