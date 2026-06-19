//! hoard-atomic — Atomic file writer for Hoard watch directories
//!
//! Reads stdin to a temp file in the target directory, fsyncs, then
//! atomically renames to the target path.  Eliminates the sendfile
//! TOCTOU race that causes ETag mismatches for overwrite workloads.
//!
//! Usage:
//!   echo "data" | hoard-atomic /var/lib/hoard/volumes/app/data.json
//!   cat payload.bin | hoard-atomic /var/lib/hoard/volumes/ingest/events.bin
//!
//! The wrapper guarantees Hoard will only ever see:
//!   - the old file (if rename hasn't happened yet), or
//!   - the new complete file (after rename)
//! It will NEVER see a half-written file.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 || args[1] == "--help" || args[1] == "-h" {
        eprintln!(
            "hoard-atomic — write stdin atomically to a file\n\
             Usage:  hoard-atomic <TARGET>\n\
             Example: echo data | hoard-atomic /var/lib/hoard/volumes/app/data.json"
        );
        process::exit(if args.len() == 2 && (args[1] == "--help" || args[1] == "-h") {
            0
        } else {
            1
        });
    }

    let target = PathBuf::from(&args[1]);

    if let Err(e) = atomic_write(&target, &mut io::stdin().lock()) {
        eprintln!("hoard-atomic: {}: {}", target.display(), e);
        process::exit(1);
    }
}

fn atomic_write(target: &Path, reader: &mut impl io::Read) -> io::Result<()> {
    let parent = target.parent().unwrap_or(Path::new("."));

    // Atomicity guarantee: create temp file in the SAME directory as target.
    // rename(2) is atomic only within the same filesystem.
    let mut tmp = parent.to_path_buf();
    tmp.push(format!(
        ".{}.hoard-tmp.{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed"),
        std::process::id()
    ));

    // O_EXCL + create_new → fail if collision (PID should make it unique)
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)?;

    // Copy stdin to temp file (kernel buffers, no userspace buffering)
    let bytes_written = io::copy(reader, &mut f)?;
    f.flush()?;

    // fsync the temp file — data-on-disk before rename
    let fd = f.as_raw_fd();
    unsafe { libc::fsync(fd) };

    // Atomic rename
    fs::rename(&tmp, target)?;

    // Best-effort fsync on the parent directory to commit the rename.
    // Not strictly required (ext4/xfs journal the rename), but belt-and-suspenders.
    let dir = File::open(parent)?;
    unsafe { libc::fsync(dir.as_raw_fd()) };

    eprintln!(
        "hoard-atomic: wrote {} bytes → {}",
        bytes_written,
        target.display()
    );
    Ok(())
}
