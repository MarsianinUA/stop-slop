//! The process around `cli::execute`: the folders of the OS, the adapter, Ctrl+C, the host, and
//! the adapter's warnings on stderr.

use std::env;
use std::ffi::OsString;
use std::io;
use std::process::{self, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};

use camino::Utf8PathBuf;
use cli::{Env, Exit};
use run::Cancel;
use sandbox::Microsandbox;
use store::{Store, StorePaths};

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().collect();
    let (mut out, mut err) = (io::stdout(), io::stderr());
    let command = match cli::parse(args.iter().cloned()) {
        Ok(command) => command,
        Err(error) => return cli::refuse_args(&error, &args, &mut out, &mut err).into(),
    };
    show_adapter_warnings();
    let cancel = Cancel::new();
    let first = AtomicBool::new(true);
    let on_ctrl_c = cancel.clone();
    // The first Ctrl+C lets the run tear its VMs down and keep what it collected; a second one
    // leaves at once, and the next start sweeps what is left. A handler that cannot be set leaves
    // Ctrl+C as it was: the signal ends the process.
    let _ = ctrlc::set_handler(move || {
        if first.swap(false, Ordering::SeqCst) {
            on_ctrl_c.cancel();
        } else {
            process::exit(Exit::Cancelled as i32);
        }
    });
    let started = paths().and_then(|(cwd, paths)| {
        let store = Store::open(paths).map_err(|error| error.to_string())?;
        let sandbox =
            Microsandbox::new(&store.sandbox_home()).map_err(|error| error.to_string())?;
        Ok((cwd, store, sandbox))
    });
    let (cwd, store, sandbox) = match started {
        Ok(started) => started,
        Err(message) => return cli::refuse_start(&command, &message, &mut out, &mut err).into(),
    };
    let env = Env {
        cwd: &cwd,
        store: &store,
        cancel: &cancel,
        host: &budget::Host::read,
        out: &mut out,
        err: &mut err,
    };
    cli::execute(command, &sandbox, env).into()
}

/// `STOP_SLOP_STAND_HOME`, or the data folder of the OS: LocalAppData on Windows, never Roaming,
/// which syncs gigabytes of snapshots with the profile (review of T-07). On macOS a short folder in
/// home: microsandbox keeps sockets under it, and `sun_path` there holds 104 bytes, which
/// `~/Library/Application Support/stop-slop-stand/msb/run/sandboxes/<24 hex>/control.sock` passes.
fn paths() -> Result<(Utf8PathBuf, StorePaths), String> {
    let utf8 = |path: std::path::PathBuf| {
        Utf8PathBuf::from_path_buf(path).map_err(|path| format!("{}: not UTF-8", path.display()))
    };
    let cwd = utf8(env::current_dir().map_err(|error| format!("the current folder: {error}"))?)?;
    let root = match env::var_os("STOP_SLOP_STAND_HOME") {
        Some(home) => utf8(home.into())?,
        None => {
            let base = if cfg!(target_os = "macos") {
                dirs::home_dir().map(|home| home.join(".stop-slop-stand"))
            } else {
                dirs::data_local_dir().map(|data| data.join("stop-slop-stand"))
            };
            utf8(base.ok_or("no home folder: set STOP_SLOP_STAND_HOME")?)?
        }
    };
    let config = dirs::config_dir().ok_or("no config folder of the OS")?;
    let config = utf8(config.join("stop-slop-stand").join("stand.toml"))?;
    Ok((cwd, StorePaths { root, config }))
}

/// The adapter logs through `tracing` and never writes to stderr itself (T-11, stage 4): its
/// warnings, such as a failed trim before a snapshot, show here. Nothing else of the SDK's logs.
fn show_adapter_warnings() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::prelude::*;

    let warnings = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .without_time()
        .with_target(false)
        .with_filter(Targets::new().with_target("sandbox", LevelFilter::WARN));
    // Nothing else sets a subscriber in this process; an error here has nowhere to go.
    let _ = tracing_subscriber::registry().with(warnings).try_init();
}
