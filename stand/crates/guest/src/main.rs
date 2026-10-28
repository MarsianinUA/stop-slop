//! `stand-guest`, run by the sandbox adapter inside each guest. It exits 125 when it fails itself,
//! with one line on stderr saying why.

#[cfg(unix)]
mod net;
#[cfg(target_os = "linux")]
mod nf;
#[cfg(unix)]
mod relay;
#[cfg(unix)]
mod sniff;

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
        Ok(_) => failed("stand-guest", "only net is built so far"),
        Err(error) => failed("stand-guest", error),
    }
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
