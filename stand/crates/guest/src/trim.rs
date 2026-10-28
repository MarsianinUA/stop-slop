//! `stand-guest trim`: the FITRIM ioctl that `fstrim` sends, on each mount of an owned disk before a
//! snapshot, so the blocks of deleted files do not travel in it. A mount that fails does not stop
//! the others, since the snapshot is taken anyway and each trimmed disk still makes it smaller;
//! each failure is a line on stderr with its path, and exit 125.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::ExitCode;

use guest::TrimArgs;

/// `struct fstrim_range` of `linux/fs.h`.
#[repr(C)]
struct FstrimRange {
    start: u64,
    len: u64,
    minlen: u64,
}

const FITRIM: libc::Ioctl = libc::_IOWR::<FstrimRange>(b'X' as u32, 121);

pub fn run(args: &TrimArgs) -> ExitCode {
    let mut code = ExitCode::SUCCESS;
    for mount in &args.mounts {
        if let Err(why) = trim(mount) {
            code = crate::failed("stand-guest trim", why);
        }
    }
    code
}

/// Every free extent of the file system that holds `mount`, whatever its length.
fn trim(mount: &Path) -> Result<(), String> {
    let file = File::open(mount).map_err(|error| format!("open {}: {error}", mount.display()))?;
    let mut range = FstrimRange {
        start: 0,
        len: u64::MAX,
        minlen: 0,
    };
    // SAFETY: FITRIM reads one `fstrim_range` and writes it back, and `range` is one.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), FITRIM, &mut range as *mut FstrimRange) };
    if rc == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // musl calls ENOTTY "Not a tty".
    let why = match error.raw_os_error() {
        Some(libc::ENOTTY) => "not supported by its file system".to_owned(),
        Some(libc::EOPNOTSUPP) => "its disk does not support discard".to_owned(),
        _ => error.to_string(),
    };
    Err(format!("FITRIM {}: {why}", mount.display()))
}
