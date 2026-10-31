//! `stand-guest trim`: the FITRIM ioctl that `fstrim` sends, on each mount of an owned disk before a
//! snapshot, so the blocks of deleted files do not travel in it. A mount that fails does not stop
//! the others, since the snapshot is taken anyway and each trimmed disk still makes it smaller;
//! each mount left untrimmed is a line on stderr with its path. The exit is 125 when any failed,
//! `TRIM_NO_DISCARD` when the only ones left are on disks that take no discard.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::ExitCode;

use guest::{TRIM_NO_DISCARD, TrimArgs};

/// `struct fstrim_range` of `linux/fs.h`.
#[repr(C)]
struct FstrimRange {
    start: u64,
    len: u64,
    minlen: u64,
}

const FITRIM: libc::Ioctl = libc::_IOWR::<FstrimRange>(b'X' as u32, 121);

pub fn run(args: &TrimArgs) -> ExitCode {
    let (mut failed, mut no_discard) = (false, false);
    for mount in &args.mounts {
        let Err(untrimmed) = trim(mount) else {
            continue;
        };
        let why = match untrimmed {
            Untrimmed::NoDiscard(why) => {
                no_discard = true;
                why
            }
            Untrimmed::Failed(why) => {
                failed = true;
                why
            }
        };
        eprintln!("stand-guest trim: {why}");
    }
    if failed {
        ExitCode::from(125)
    } else if no_discard {
        ExitCode::from(TRIM_NO_DISCARD)
    } else {
        ExitCode::SUCCESS
    }
}

enum Untrimmed {
    NoDiscard(String),
    Failed(String),
}

/// Every free extent of the file system that holds `mount`, whatever its length.
fn trim(mount: &Path) -> Result<(), Untrimmed> {
    let file = File::open(mount)
        .map_err(|error| Untrimmed::Failed(format!("open {}: {error}", mount.display())))?;
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
    let said = |why: &str| format!("FITRIM {}: {why}", mount.display());
    // musl calls ENOTTY "Not a tty".
    Err(match error.raw_os_error() {
        Some(libc::ENOTTY) => Untrimmed::Failed(said("not supported by its file system")),
        Some(libc::EOPNOTSUPP) => Untrimmed::NoDiscard(said("its disk does not support discard")),
        _ => Untrimmed::Failed(said(&error.to_string())),
    })
}
