use std::io::{self, BufWriter, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use gitcmd::Git;
use tar::{Builder, EntryType, Header};

use crate::{SourceError, Tree, unexpected};

/// Blob bytes asked of one `cat-file --batch`: memory stays near one batch, and a big tree costs
/// few starts of git. A bigger blob is a batch of its own.
const BATCH: usize = 32 << 20;

/// The name and link fields of a header. GNU tar puts a name or a link target this long into an
/// `L` or `K` entry before the header, and so does this module.
const FIELD: usize = 100;

struct Entry<'a> {
    kind: Kind,
    oid: &'a str,
    size: usize,
    path: &'a [u8],
}

enum Kind {
    File { mode: u32 },
    Link,
}

pub(crate) fn write(git: &Git, tree: &Tree, out: &mut dyn Write) -> Result<(), SourceError> {
    let listing = git.run(&["ls-tree", "-r", "-z", "--long", "--full-tree", &tree.0])?;
    let entries = parse(&listing)?;
    // As `git archive` dates the files of a tree and as a checkout leaves them; files dated
    // 1970 break build steps that zip them (zip has no dates before 1980).
    let mtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut builder = Builder::new(BufWriter::new(out));
    for batch in batches(&entries) {
        let requests: String = batch
            .iter()
            .map(|entry| format!("{}\n", entry.oid))
            .collect();
        // Without `--buffer` git flushes its output after every object.
        let blobs =
            git.run_with_input(&["cat-file", "--batch", "--buffer"], requests.as_bytes())?;
        let mut rest = blobs.as_slice();
        for entry in batch {
            let blob = next_blob(&mut rest, entry)?;
            append(&mut builder, entry, blob, mtime).map_err(SourceError::Write)?;
        }
    }
    builder
        .into_inner()
        .and_then(|mut out| out.flush())
        .map_err(SourceError::Write)
}

/// `<mode> <type> <oid> <size>\t<path>\0` per entry; the path is git's bytes.
fn parse(listing: &[u8]) -> Result<Vec<Entry<'_>>, SourceError> {
    let bad = || unexpected("ls-tree");
    let mut entries = Vec::new();
    for record in listing
        .split(|&byte| byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|&byte| byte == b'\t')
            .ok_or_else(bad)?;
        let fields = str::from_utf8(&record[..tab]).map_err(|_| bad())?;
        let mut fields = fields.split_whitespace();
        let (Some(mode), Some(object), Some(oid), Some(size)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(bad());
        };
        let kind = match (mode, object) {
            ("100644", "blob") => Kind::File { mode: 0o644 },
            ("100755", "blob") => Kind::File { mode: 0o755 },
            ("120000", "blob") => Kind::Link,
            // A submodule has no blob in this repo, and folders come only from paths of files.
            ("160000", "commit") => continue,
            _ => return Err(bad()),
        };
        entries.push(Entry {
            kind,
            oid,
            size: size.parse().map_err(|_| bad())?,
            path: &record[tab + 1..],
        });
    }
    Ok(entries)
}

fn batches<'a, 'b>(entries: &'b [Entry<'a>]) -> Vec<&'b [Entry<'a>]> {
    let mut batches = Vec::new();
    let mut rest = entries;
    while let Some(first) = rest.first() {
        let mut bytes = first.size;
        let mut end = 1;
        while let Some(next) = rest.get(end)
            && bytes + next.size <= BATCH
        {
            bytes += next.size;
            end += 1;
        }
        let (batch, tail) = rest.split_at(end);
        batches.push(batch);
        rest = tail;
    }
    batches
}

/// The answer to the request of `entry`: `<oid> blob <size>\n<bytes>\n`.
fn next_blob<'a>(rest: &mut &'a [u8], entry: &Entry) -> Result<&'a [u8], SourceError> {
    let bad = || unexpected("cat-file --batch");
    let header = format!("{} blob {}\n", entry.oid, entry.size);
    let after = rest.strip_prefix(header.as_bytes()).ok_or_else(bad)?;
    let (blob, tail) = after.split_at_checked(entry.size).ok_or_else(bad)?;
    *rest = tail.strip_prefix(b"\n").ok_or_else(bad)?;
    Ok(blob)
}

/// A file with its mode, or a link whose target is the blob. No folder entries: `tar -x`
/// creates the folders on the way to a file.
fn append(
    builder: &mut Builder<impl Write>,
    entry: &Entry,
    blob: &[u8],
    mtime: u64,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_mtime(mtime);
    header.set_uid(0);
    header.set_gid(0);
    let data = match entry.kind {
        Kind::File { mode } => {
            header.set_entry_type(EntryType::Regular);
            header.set_mode(mode);
            header.set_size(blob.len() as u64);
            blob
        }
        Kind::Link => {
            long_name(builder, b'K', blob)?;
            header.set_entry_type(EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            fill(&mut gnu(&mut header).linkname, blob);
            &[]
        }
    };
    long_name(builder, b'L', entry.path)?;
    fill(&mut gnu(&mut header).name, entry.path);
    header.set_cksum();
    builder.append(&header, data)
}

/// The `././@LongLink` entry of GNU tar that carries a long `name` for the next header. The
/// headers take bytes, not `Path`: the `tar` crate turns `\` into `/` and reads `c:x` as a drive
/// on Windows, and a tree must give the same tar on every host.
fn long_name(builder: &mut Builder<impl Write>, kind: u8, name: &[u8]) -> io::Result<()> {
    if name.len() < FIELD {
        return Ok(());
    }
    let mut header = Header::new_gnu();
    fill(&mut gnu(&mut header).name, b"././@LongLink");
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_entry_type(EntryType::new(kind));
    // The name ends with a NUL inside the entry, as GNU tar writes it.
    header.set_size(name.len() as u64 + 1);
    header.set_cksum();
    builder.append(&header, name.chain(&[0][..]))
}

fn gnu(header: &mut Header) -> &mut tar::GnuHeader {
    header
        .as_gnu_mut()
        .expect("a header from `Header::new_gnu`")
}

/// As much of `bytes` as fits; a longer value is in the entry before the header.
fn fill(field: &mut [u8; FIELD], bytes: &[u8]) {
    let len = bytes.len().min(FIELD);
    field[..len].copy_from_slice(&bytes[..len]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(size: usize) -> Entry<'static> {
        Entry {
            kind: Kind::File { mode: 0o644 },
            oid: "",
            size,
            path: b"",
        }
    }

    #[test]
    fn a_batch_holds_up_to_its_bytes_and_a_bigger_blob_goes_alone() {
        let entries = [10, BATCH - 10, 1, BATCH + 5, 0, 3].map(sized);
        let sizes: Vec<Vec<usize>> = batches(&entries)
            .iter()
            .map(|batch| batch.iter().map(|entry| entry.size).collect())
            .collect();
        assert_eq!(
            sizes,
            [vec![10, BATCH - 10], vec![1], vec![BATCH + 5], vec![0, 3]]
        );
        assert!(batches(&[]).is_empty());
    }
}
