//! The writable overlay, over a real XFS underlay.
//!
//! XFS is mounted read-only here and every write is staged in memory, so
//! these check the thing that actually matters about the overlay: that a
//! staged change is visible to subsequent reads **and that the image on
//! disk is untouched**. The second half is the safety property — a
//! driver that quietly wrote through would pass the first half alone.
//!
//! REWRITTEN 2026-09-04. The previous version built its underlay with
//! `fs_xfs::mkfs::build_image`, a module that does not exist — creating
//! an XFS filesystem from nothing is unfinished work in the reader — and
//! the whole file was gated off behind a feature nothing enabled. It
//! came from `erofs-win-driver`, where that module does exist.
//!
//! Two tests are gone rather than rewritten: `dismount_rebuild_*`
//! exercised `DismountPolicy::Rebuild`, which serialises the overlay
//! into a fresh image through the reader's `mkfs`. That policy is
//! removed from this driver for the same reason, so there is nothing
//! left for them to test. They come back with `mkfs.xfs`.

mod common;

use common::{fixture_or_skip, has_our_content, mount};
use xfs_win_driver::mount::{DismountPolicy, Mount};

/// A path the fixture is known to contain, with its bytes. Tests that
/// need specific content use this; tests that only need "some file"
/// find one by walking.
const KNOWN_FILE: &str = "/small.txt";
const KNOWN_BYTES: &[u8] = b"hello xfs";

/// Open the fixture as a mount, skipping with a reason when there is no
/// image or when it lacks the content the script writes.
fn open_fixture() -> Option<(Mount, std::path::PathBuf)> {
    let img = fixture_or_skip()?;
    let fs = mount(&img);
    if !has_our_content(&fs) {
        eprintln!("SKIPPED: fixture lacks the deliberate content; run scripts/build-fixtures.sh");
        return None;
    }
    let m = Mount::open_direct(&img).expect("open the fixture");
    Some((m, img))
}

#[test]
fn an_unmodified_file_reads_from_the_underlay() {
    let Some((m, _)) = open_fixture() else { return };
    assert_eq!(
        m.read_path(KNOWN_FILE).expect("read"),
        KNOWN_BYTES,
        "with nothing staged, a read must come straight from the image"
    );
}

/// The central property: a staged write is visible through the mount,
/// and the image on disk still holds the original bytes.
#[test]
fn a_staged_write_is_visible_but_the_image_is_untouched() {
    let Some((m, img)) = open_fixture() else {
        return;
    };

    // Mirrors the WinFsp sequence: truncate, then write.
    m.set_size_path(KNOWN_FILE, 0).expect("truncate");
    m.write_path(KNOWN_FILE, 0, b"OVERWRITTEN").expect("write");
    assert_eq!(
        m.read_path(KNOWN_FILE).expect("read back"),
        b"OVERWRITTEN",
        "the staged bytes should be what a subsequent read sees"
    );

    // Re-open the image independently. This is the assertion that makes
    // the overlay worth having: if the driver wrote through, the file on
    // disk would have changed.
    let untouched = mount(&img);
    let on_disk = untouched
        .open(KNOWN_FILE)
        .expect("the file is still there")
        .read_all()
        .expect("read it from disk");
    assert_eq!(
        on_disk, KNOWN_BYTES,
        "the underlay image was modified — the overlay is supposed to be in memory only"
    );
}

#[test]
fn a_created_file_is_readable_and_absent_from_the_underlay() {
    let Some((m, _)) = open_fixture() else { return };

    m.overlay
        .create_file("/fresh.txt", b"fresh content".to_vec(), 0o100644);
    assert_eq!(
        m.read_path("/fresh.txt").expect("read the created file"),
        b"fresh content"
    );
    assert!(
        m.fs.open("/fresh.txt").is_err(),
        "a file that only exists in the overlay must not resolve in the underlay"
    );
}

#[test]
fn a_deleted_file_stops_resolving() {
    let Some((m, img)) = open_fixture() else {
        return;
    };

    m.overlay.delete(KNOWN_FILE);
    assert_eq!(
        m.read_path(KNOWN_FILE).unwrap_err(),
        "not found",
        "a tombstoned path must read as missing, not as its underlay content"
    );

    // And the image still has it.
    let untouched = mount(&img);
    assert!(
        untouched.open(KNOWN_FILE).is_ok(),
        "deleting through the overlay must not remove the file from the image"
    );
}

/// Renaming a file that exists ONLY in the underlay. This is the common
/// case in practice and the one the first version of this test got
/// wrong: it called `Overlay::rename`, which by design refuses a source
/// it has no entry for, and the failure was the test's, not the
/// driver's. `Mount::rename_path` is the operation that promotes from
/// the underlay -- it used to live inside the `cfg(windows)` callback
/// where no test here could reach it.
#[test]
fn renaming_an_underlay_file_carries_its_content() {
    let Some((m, img)) = open_fixture() else {
        return;
    };

    m.rename_path(KNOWN_FILE, "/renamed.txt", false)
        .expect("rename a file that exists only in the image");
    assert_eq!(
        m.read_path("/renamed.txt").expect("read at the new name"),
        KNOWN_BYTES,
        "the content should be promoted out of the underlay, not lost"
    );
    assert!(
        m.read_path(KNOWN_FILE).is_err(),
        "the old name must stop resolving"
    );

    // And the image is untouched: the old name is still in it.
    let untouched = mount(&img);
    assert!(
        untouched.open(KNOWN_FILE).is_ok(),
        "renaming through the overlay must not modify the image"
    );
    assert!(
        untouched.open("/renamed.txt").is_err(),
        "the new name must not appear in the image"
    );
}

/// A destination that already exists is refused unless the caller says
/// to replace it -- the `replace_if_exists` flag WinFsp passes through.
#[test]
fn rename_onto_an_existing_name_needs_permission() {
    let Some((m, _)) = open_fixture() else { return };

    assert!(
        m.rename_path(KNOWN_FILE, "/empty.txt", false).is_err(),
        "clobbering an existing file must not be the default"
    );
    // The refusal left nothing staged: both names still read as before.
    assert_eq!(m.read_path(KNOWN_FILE).expect("source intact"), KNOWN_BYTES);
    assert_eq!(
        m.read_path("/empty.txt").expect("destination intact"),
        Vec::<u8>::new()
    );

    m.rename_path(KNOWN_FILE, "/empty.txt", true)
        .expect("with permission it goes through");
    assert_eq!(
        m.read_path("/empty.txt").expect("read the replaced file"),
        KNOWN_BYTES
    );
}

/// A tombstoned destination is a free name. Checking the underlay
/// without consulting the overlay first would see the deleted file and
/// refuse -- resurrecting a name the caller had already removed.
#[test]
fn rename_onto_a_deleted_name_is_allowed() {
    let Some((m, _)) = open_fixture() else { return };

    m.overlay.delete("/empty.txt");
    m.rename_path(KNOWN_FILE, "/empty.txt", false)
        .expect("a tombstoned name is free even though the image still has it");
    assert_eq!(m.read_path("/empty.txt").expect("read"), KNOWN_BYTES);
}

/// Renaming a file already staged in the overlay moves the staged
/// bytes, not the underlay's.
#[test]
fn rename_moves_staged_content_not_underlay_content() {
    let Some((m, _)) = open_fixture() else { return };

    m.set_size_path(KNOWN_FILE, 0).expect("truncate");
    m.write_path(KNOWN_FILE, 0, b"staged").expect("stage");
    m.rename_path(KNOWN_FILE, "/moved.txt", false)
        .expect("rename");

    assert_eq!(
        m.read_path("/moved.txt").expect("read"),
        b"staged",
        "the rename must carry the staged bytes, not re-read the image"
    );
}

/// Renaming a path that is not there fails rather than staging an empty
/// file at the destination.
#[test]
fn renaming_a_missing_path_stages_nothing() {
    let Some((m, _)) = open_fixture() else { return };

    assert!(m.rename_path("/not-here", "/somewhere", false).is_err());
    assert!(
        m.read_path("/somewhere").is_err(),
        "a failed rename must not leave a destination behind"
    );
}

/// Renaming to the same path is a no-op rather than a self-tombstone.
/// Getting this wrong deletes the file: stage at `to`, then delete
/// `from`, and when they are equal the delete wins.
#[test]
fn renaming_a_path_onto_itself_keeps_it() {
    let Some((m, _)) = open_fixture() else { return };

    m.rename_path(KNOWN_FILE, KNOWN_FILE, false)
        .expect("a no-op rename succeeds");
    assert_eq!(
        m.read_path(KNOWN_FILE).expect("the file is still there"),
        KNOWN_BYTES,
        "renaming a path to itself must not delete it"
    );
}

#[test]
fn discarding_on_dismount_drops_every_staged_change() {
    let Some((m, _)) = open_fixture() else { return };

    m.write_path(KNOWN_FILE, 0, b"XX").expect("stage a write");
    m.overlay
        .create_file("/gone.txt", b"vanishes".to_vec(), 0o100644);

    let m = m.with_dismount_policy(DismountPolicy::Discard);
    m.apply_dismount_policy().expect("discard");

    assert_eq!(
        m.read_path(KNOWN_FILE).expect("read after discard"),
        KNOWN_BYTES,
        "a discarded write should leave the underlay content showing"
    );
    assert!(
        m.read_path("/gone.txt").is_err(),
        "a discarded creation should not survive"
    );
}

/// Nested paths, so the overlay is exercised past a single directory
/// level — a path-joining bug shows up here and not in the root.
#[test]
fn the_overlay_handles_nested_paths() {
    let Some((m, _)) = open_fixture() else { return };
    const NESTED: &str = "/dir/nested/deep/leaf.txt";

    assert_eq!(m.read_path(NESTED).expect("read nested"), b"level three");

    m.set_size_path(NESTED, 0).expect("truncate nested");
    m.write_path(NESTED, 0, b"replaced").expect("write nested");
    assert_eq!(m.read_path(NESTED).expect("read back"), b"replaced");

    // A sibling in the same directory is unaffected.
    assert_eq!(
        m.read_path("/dir/one.txt").expect("read the sibling"),
        b"level one",
        "staging one file must not disturb another in the same directory"
    );
}

/// A write past the end of a file zero-fills the gap rather than leaving
/// whatever was in memory.
#[test]
fn writing_past_the_end_zero_fills() {
    let Some((m, _)) = open_fixture() else { return };

    m.write_path(KNOWN_FILE, 32, b"far")
        .expect("write past EOF");
    let content = m.read_path(KNOWN_FILE).expect("read back");

    assert_eq!(content.len(), 35, "the file should have grown to fit");
    assert_eq!(
        &content[..KNOWN_BYTES.len()],
        KNOWN_BYTES,
        "the original bytes stay"
    );
    assert!(
        content[KNOWN_BYTES.len()..32].iter().all(|&b| b == 0),
        "the gap must be zeros, not uninitialised memory"
    );
    assert_eq!(&content[32..], b"far");
}

/// Writing to a directory is refused rather than corrupting the overlay.
#[test]
fn writing_to_a_directory_is_refused() {
    let Some((m, _)) = open_fixture() else { return };
    assert!(
        m.write_path("/dir", 0, b"nope").is_err(),
        "a directory is not writable as a file"
    );
}
