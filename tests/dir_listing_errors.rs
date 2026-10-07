//! A directory the reader cannot list is an error, never an empty folder.
//!
//! The WinFsp `read_directory` callback once listed a directory with
//! `if let Ok(children) = ...`, so a directory whose data could not be
//! read came back to Windows as a folder with nothing in it, and a child
//! whose inode could not be read silently dropped out of the listing.
//! Seen on a real volume: `X:\usr\bin` listed as empty while the reader
//! underneath refused it. An empty folder is the one answer a user cannot
//! tell from a correct one.
//!
//! These read the kernel-built fixture through a device that starts
//! failing on demand, so the failure is a real read error from the real
//! read path rather than a fake filesystem.

// This file does not use every helper in `common`.
#[allow(dead_code)]
mod common;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{fixture_or_skip, has_our_content};
use fs_core::{BlockRead, FileDevice};
use fs_xfs::Filesystem;
use xfs_win_driver::mount::{ntstatus, ntstatus_for, underlay_children};
use xfs_win_driver::overlay::Overlay;

/// The fixture, read through a switch: once `fail_all` is set, or for
/// any read overlapping `fail_range`, the read fails as the device would.
struct FailingDevice {
    inner: FileDevice,
    fail_all: AtomicBool,
    fail_range: Mutex<Option<Range<u64>>>,
}

impl BlockRead for FailingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        let end = offset + buf.len() as u64;
        let in_range = self
            .fail_range
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| offset < r.end && r.start < end);
        if self.fail_all.load(Ordering::SeqCst) || in_range {
            return Err(fs_core::Error::Io(std::io::Error::other(
                "injected read failure",
            )));
        }
        self.inner.read_at(offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

/// The fixture through a `FailingDevice`, with no block cache: a cached
/// block would answer a read the test means to fail.
fn open() -> Option<(Arc<FailingDevice>, Filesystem)> {
    let img = fixture_or_skip()?;
    let dev = Arc::new(FailingDevice {
        inner: FileDevice::open(&img).expect("open the fixture image"),
        fail_all: AtomicBool::new(false),
        fail_range: Mutex::new(None),
    });
    let fs = Filesystem::mount_with_cache(dev.clone() as Arc<dyn BlockRead>, 0)
        .expect("mount the fixture image");
    if !has_our_content(&fs) {
        eprintln!("SKIPPED: fixture lacks the deliberate content; run scripts/build-fixtures.sh");
        return None;
    }
    Some((dev, fs))
}

/// With nothing failing, every one of the 200 entries
/// `build-fixtures.sh` made in `/manyentries` is listed.
#[test]
fn a_readable_directory_lists_every_entry() {
    let Some((_dev, fs)) = open() else { return };
    let dir = fs
        .lookup_path("/manyentries")
        .expect("look up /manyentries");
    let listed = underlay_children(&fs, &Overlay::new(), &dir, "/manyentries")
        .expect("a readable directory lists");
    assert_eq!(listed.len(), 200, "every entry the fixture made");
}

/// A directory whose contents cannot be read is an I/O error, not an
/// empty listing.
#[test]
fn an_unreadable_directory_is_an_io_error_not_an_empty_folder() {
    let Some((dev, fs)) = open() else { return };
    let dir = fs
        .lookup_path("/manyentries")
        .expect("look up /manyentries");
    dev.fail_all.store(true, Ordering::SeqCst);

    let got = underlay_children(&fs, &Overlay::new(), &dir, "/manyentries");
    let err = match got {
        Ok(entries) => panic!(
            "an unreadable directory was listed as {} entries instead of failing",
            entries.len()
        ),
        Err(e) => e,
    };
    assert_eq!(
        ntstatus_for(&err),
        ntstatus::IO_ERROR,
        "a read failure reaches Windows as STATUS_IO_ERROR, got {err}"
    );
}

/// A child whose inode cannot be read fails the listing rather than
/// vanishing from it.
#[test]
fn an_unreadable_child_fails_the_listing_rather_than_vanishing() {
    let Some((dev, fs)) = open() else { return };
    let dir = fs
        .lookup_path("/manyentries")
        .expect("look up /manyentries");
    let entries = fs.list_path("/manyentries").expect("list /manyentries");

    // A child whose inode is not in the same filesystem block as the
    // directory's own, so failing it leaves the directory readable and
    // only the per-child read can fail.
    let bs = u64::from(fs.superblock().blocksize);
    let isize = u64::from(fs.superblock().inodesize);
    let dir_block = fs.inode_offset(dir.ino).unwrap() / bs;
    let victim = entries
        .iter()
        .filter(|e| e.name != b"." && e.name != b"..")
        .map(|e| fs.inode_offset(e.ino).unwrap())
        .find(|&off| off / bs != dir_block)
        .expect("a child inode outside the directory's own block");
    *dev.fail_range.lock().unwrap() = Some(victim..victim + isize);

    let got = underlay_children(&fs, &Overlay::new(), &dir, "/manyentries");
    let err = match got {
        Ok(listed) => panic!(
            "a child that could not be read was dropped: {} of 200 entries listed, no error",
            listed.len()
        ),
        Err(e) => e,
    };
    assert_eq!(
        ntstatus_for(&err),
        ntstatus::IO_ERROR,
        "a read failure reaches Windows as STATUS_IO_ERROR, got {err}"
    );
}
