//! Finding the XFS image the tests read, and saying so when there isn't
//! one.
//!
//! The images are real filesystems built by `mkfs.xfs` and populated by
//! the kernel — see `scripts/build-fixtures.sh`, which lists what is in
//! them and why. They are gitignored, because a 300 MiB binary does not
//! belong in a repository, so a fresh clone has none.
//!
//! That makes "no fixture" a normal state rather than a failure, and it
//! creates the trap this module exists to avoid: **a test that skips
//! silently passes while proving nothing.** Every skip here prints why,
//! and the tests that exist to measure something specific are written to
//! fail rather than skip once a fixture *is* present.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs_core::{BlockRead, FileDevice};
use fs_xfs::Filesystem;

/// Where an image might be, in the order worth trying.
///
/// 1. `XFS_TEST_IMAGE` — an explicit override, and what CI sets.
/// 2. `.fixtures/` — where `scripts/build-fixtures.sh` writes.
/// 3. The sibling reader's `.vm-share/` — convenience for a developer
///    who has already built fixtures over there and should not have to
///    build them twice. Only images with content are accepted, so the
///    bare geometry fixtures in that directory are skipped rather than
///    yielding a filesystem with an empty root.
pub fn fixture() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("XFS_TEST_IMAGE") {
        let p = PathBuf::from(explicit);
        return p.exists().then_some(p);
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let own = root.join(".fixtures/xfs-content.img");
    if own.exists() {
        return Some(own);
    }

    let sibling = root.join("../rust-fs-xfs/.vm-share");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(sibling)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "img"))
        .collect();
    candidates.sort();
    candidates.into_iter().find(|p| has_content(p))
}

/// Whether an image's root holds anything. Guards against the bare
/// `mkfs.xfs` fixtures, which mount fine and contain nothing — a test
/// handed one finds no files to assert on and passes vacuously.
fn has_content(img: &Path) -> bool {
    let Ok(dev) = FileDevice::open(img) else {
        return false;
    };
    let Ok(fs) = Filesystem::mount(Arc::new(dev) as Arc<dyn BlockRead>) else {
        return false;
    };
    let Ok(root) = fs.root() else { return false };
    root.entries()
        .is_ok_and(|es| es.iter().any(|e| e.name != b"." && e.name != b".."))
}

/// The message every skip prints. One wording, so a run's output makes
/// it obvious that tests were skipped rather than passed.
pub fn skip_reason() -> String {
    format!(
        "SKIPPED: no XFS fixture. Build one with `sudo scripts/build-fixtures.sh` \
         (needs mkfs.xfs, so Linux), or set XFS_TEST_IMAGE. Looked in \
         $XFS_TEST_IMAGE, {}/.fixtures/, and ../rust-fs-xfs/.vm-share/.",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Fetch a fixture or explain the skip. Returns `None` after printing.
#[must_use]
pub fn fixture_or_skip() -> Option<PathBuf> {
    match fixture() {
        Some(p) => Some(p),
        None => {
            eprintln!("{}", skip_reason());
            None
        }
    }
}

/// Whether the deliberate content from `build-fixtures.sh` is present.
///
/// The sibling `.vm-share` images have content but not *our* content, so
/// a test asserting on `small.txt` must know which kind it has. Tests
/// that need the specific files check this and skip otherwise; tests
/// that only need "a populated filesystem" do not.
pub fn has_our_content(fs: &Filesystem) -> bool {
    fs.open("/small.txt").is_ok() && fs.open("/pattern.bin").is_ok()
}

/// Mount a fixture for reading.
pub fn mount(img: &Path) -> Filesystem {
    let dev = FileDevice::open(img).expect("open the fixture image");
    Filesystem::mount(Arc::new(dev) as Arc<dyn BlockRead>).expect("mount the fixture image")
}
