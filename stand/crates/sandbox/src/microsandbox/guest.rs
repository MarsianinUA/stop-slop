//! Which `stand-guest` goes to `/stand/bin`, and what its `net` counted.
//!
//! The pinned release by default, checked against its sha256; with `STOP_SLOP_STAND_GUEST` the file
//! it names, never downloaded and never checked. The pin moves with each `guest-v*` tag (plan,
//! "`guest`: релизы").

use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};

use super::{backend, guest_error};
use crate::{PortStats, SandboxError, Unexposed};

/// In the guest, bound read-only from `Binary::dir`.
pub(super) const DIR: &str = "/stand/bin";
pub(super) const BIN: &str = "/stand/bin/stand-guest";
const FILE: &str = "stand-guest";
const RELEASES: &str = "https://github.com/MarsianinUA/stop-slop/releases/download";
const LOCAL_BUILD: &str = "STOP_SLOP_STAND_GUEST";

struct Pin {
    version: &'static str,
    x86_64: &'static str,
    aarch64: &'static str,
}

/// The commit `Pin stand-guest 0.1.0` puts the first field of each `.sha256` of the release here;
/// until then only `STOP_SLOP_STAND_GUEST` works.
const PIN: Pin = Pin {
    version: "0.1.0",
    x86_64: "0000000000000000000000000000000000000000000000000000000000000000",
    aarch64: "0000000000000000000000000000000000000000000000000000000000000000",
};

/// `version` goes into `Pulled.guest`.
#[derive(Debug, Clone)]
pub(super) struct Binary {
    pub dir: Utf8PathBuf,
    pub version: String,
}

pub(super) fn local_build() -> Option<Utf8PathBuf> {
    std::env::var_os(LOCAL_BUILD).map(|path| Utf8PathBuf::from(path.to_string_lossy().into_owned()))
}

enum Source {
    Local(Utf8PathBuf),
    Release { url: String, sha256: &'static str },
}

/// The guest runs the host's architecture, as microsandbox boots it.
fn source(local: Option<Utf8PathBuf>, arch: &str) -> Result<Source, SandboxError> {
    if let Some(path) = local {
        return Ok(Source::Local(path));
    }
    let sha256 = match arch {
        "x86_64" => PIN.x86_64,
        "aarch64" => PIN.aarch64,
        other => return Err(guest_error(format!("no stand-guest build for {other}"))),
    };
    let url = format!(
        "{RELEASES}/guest-v{}/stand-guest-{arch}-unknown-linux-musl",
        PIN.version
    );
    Ok(Source::Release { url, sha256 })
}

/// Into `<home>/guest/<version>/`, kept there for later runs.
pub(super) async fn install(
    home: &Utf8Path,
    local: Option<Utf8PathBuf>,
) -> Result<Binary, SandboxError> {
    let dir_of = |version: &str| home.join("guest").join(version);
    // Only what is not in place yet: a running VM may have the file open through its bind, and
    // Windows refuses to replace it.
    let (version, missing) = match source(local, std::env::consts::ARCH)? {
        Source::Local(path) => {
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|error| guest_error(format!("{LOCAL_BUILD}={path}: {error}")))?;
            let version = format!("dev-{}", hex(&Sha256::digest(&bytes)));
            // Named by its sha256, and placed whole or not at all.
            let placed = dir_of(&version).join(FILE).exists();
            (version, (!placed).then_some(bytes))
        }
        Source::Release { url, sha256 } => {
            let placed = tokio::fs::read(dir_of(PIN.version).join(FILE)).await;
            let missing = match placed {
                Ok(bytes) if checked(&bytes, sha256).is_ok() => None,
                _ => {
                    let bytes = download(&url).await?;
                    checked(&bytes, sha256)?;
                    Some(bytes)
                }
            };
            (PIN.version.to_owned(), missing)
        }
    };
    let dir = dir_of(&version);
    if let Some(bytes) = missing {
        place(&dir, &bytes)?;
    }
    Ok(Binary { dir, version })
}

async fn download(url: &str) -> Result<Vec<u8>, SandboxError> {
    let failed = |error: reqwest::Error| guest_error(format!("download {url}: {error}"));
    let response = reqwest::get(url).await.map_err(failed)?;
    let bytes = response.error_for_status().map_err(failed)?.bytes().await;
    Ok(bytes.map_err(failed)?.to_vec())
}

fn checked(bytes: &[u8], sha256: &str) -> Result<(), SandboxError> {
    let got = hex(&Sha256::digest(bytes));
    if got == sha256 {
        Ok(())
    } else {
        Err(guest_error(format!(
            "stand-guest has sha256 {got}, the pin says {sha256}"
        )))
    }
}

/// Through a temporary name: a half-written binary in the bind would be run by the next VM.
fn place(dir: &Utf8Path, bytes: &[u8]) -> Result<(), SandboxError> {
    let failed = |error: std::io::Error| backend(format!("{dir}: {error}"));
    std::fs::create_dir_all(dir).map_err(failed)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, bytes).map_err(failed)?;
    // A downloaded asset has no exec bit.
    super::set_mode(&tmp, 0o755).map_err(failed)?;
    std::fs::rename(&tmp, dir.join(FILE)).map_err(failed)
}

/// What `net` counted so far. `net` writes the file before the service starts, so a missing one
/// means it is broken.
pub(super) fn traffic(
    counters: &Utf8Path,
) -> Result<(Vec<PortStats>, Vec<Unexposed>), SandboxError> {
    let text = std::fs::read_to_string(counters)
        .map_err(|error| guest_error(format!("{counters}: {error}")))?;
    let counters = ::guest::Counters::parse(&text)
        .map_err(|error| guest_error(format!("{counters}: {error}")))?;
    let ports = counters
        .targets
        .into_iter()
        .map(|target| PortStats {
            target: target.target,
            connections: target.connections,
            open: target.open,
            bytes_out: target.bytes_out,
            bytes_in: target.bytes_in,
            http: target.http,
            drops: target.drops,
        })
        .collect();
    let unexposed = counters
        .unexposed
        .into_iter()
        .map(|count| Unexposed {
            name: count.name,
            attempts: count.attempts,
        })
        .collect();
    Ok((ports, unexposed))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_foreign_sha256_is_a_guest_error() {
        let pinned = hex(&Sha256::digest(b"the pinned build"));
        assert!(checked(b"the pinned build", &pinned).is_ok());
        let foreign = checked(b"another build", &pinned);
        assert!(
            matches!(foreign, Err(SandboxError::Guest { .. })),
            "{foreign:?}"
        );
    }

    #[test]
    fn a_local_build_is_taken_without_a_download() {
        let local = Utf8PathBuf::from("/builds/stand-guest");
        let taken = source(Some(local.clone()), "x86_64");
        assert!(matches!(taken, Ok(Source::Local(path)) if path == local));
        let pinned = source(None, "x86_64");
        let release = "stand-guest-x86_64-unknown-linux-musl";
        assert!(matches!(pinned, Ok(Source::Release { url, .. }) if url.ends_with(release)));
    }
}
