//! Overlay integration tests.
//!
//! GATED OFF UNTIL `am-fs-xfs` CAN FORMAT. Every fixture in this file is
//! built with `fs_xfs::mkfs::build_image`, and that module does not
//! exist: creating an XFS filesystem from nothing is unfinished work in
//! the reader — the superblock writer landed, the allocation group
//! headers, btrees, root inode and log have not.
//!
//! Kept rather than deleted, because the tests themselves are sound and
//! the only thing missing is a way to produce an image to run them
//! against. When `mkfs.xfs` exists, delete the `cfg` below and they
//! should build unchanged — the feature it names is deliberately one
//! that no `Cargo.toml` defines, so the file compiles to nothing today
//! and cannot be enabled by accident.
#![cfg(feature = "xfs-mkfs-fixtures")]

use std::collections::BTreeMap;
use std::io::Write;

use fs_xfs::mkfs::{build_image, Node, NodeMeta, DEFAULT_DIR_MODE, DEFAULT_FILE_MODE};
use fs_xfs::Filesystem;

use xfs_win_driver::mount::{DismountPolicy, Mount};

/// Build a small XFS image with:
/// - `/hello.txt` → "hi from underlay\n"
/// - `/dir/inner.txt` → "inner\n"
///
/// Returns the raw bytes ready to be written to a tempfile.
fn build_underlay_image() -> Vec<u8> {
    let mut root_entries: BTreeMap<String, Node> = BTreeMap::new();
    root_entries.insert(
        "hello.txt".into(),
        Node::File {
            mode: DEFAULT_FILE_MODE,
            data: b"hi from underlay\n".to_vec(),
            meta: NodeMeta::default(),
            xattrs: Vec::new(),
        },
    );
    let mut dir_entries: BTreeMap<String, Node> = BTreeMap::new();
    dir_entries.insert(
        "inner.txt".into(),
        Node::File {
            mode: DEFAULT_FILE_MODE,
            data: b"inner\n".to_vec(),
            meta: NodeMeta::default(),
            xattrs: Vec::new(),
        },
    );
    root_entries.insert(
        "dir".into(),
        Node::Dir {
            mode: DEFAULT_DIR_MODE,
            entries: dir_entries,
            meta: NodeMeta::default(),
            xattrs: Vec::new(),
        },
    );
    build_image(
        Node::Dir {
            mode: DEFAULT_DIR_MODE,
            entries: root_entries,
            meta: NodeMeta::default(),
            xattrs: Vec::new(),
        },
        12,
    )
    .expect("build_image")
}

fn write_tempimg(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut tf = tempfile::NamedTempFile::new().expect("tempfile");
    tf.write_all(bytes).expect("write");
    tf.flush().expect("flush");
    tf
}

#[test]
fn read_existing_underlay_file_returns_underlay_content() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let m = Mount::open_direct(tf.path()).expect("open");
    let content = m.read_path("/hello.txt").expect("read /hello.txt");
    assert_eq!(content, b"hi from underlay\n".to_vec());
}

#[test]
fn write_to_existing_file_overrides_underlay_for_subsequent_reads() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let m = Mount::open_direct(tf.path()).expect("open");

    // Mirror the WinFsp create-truncating-write sequence: shrink to
    // zero first, then write the new bytes. The native `write`
    // callback patches in place; truncation comes through
    // `set_file_size` (or the create-replacing flag).
    m.set_size_path("/hello.txt", 0).expect("truncate");
    m.write_path("/hello.txt", 0, b"OVERWRITTEN\n")
        .expect("write");
    let content = m.read_path("/hello.txt").expect("read after write");
    assert_eq!(content, b"OVERWRITTEN\n".to_vec());

    // Re-open the underlay independently — its bytes must be unchanged.
    let dev = fs_core::FileDevice::open(tf.path()).expect("reopen device");
    use std::sync::Arc;
    let dev: Arc<dyn fs_core::BlockRead> = Arc::new(dev);
    let underlay = Filesystem::open(dev).expect("reopen underlay");
    let inode = underlay.lookup_path("/hello.txt").expect("underlay lookup");
    let mut buf = vec![0u8; inode.size as usize];
    underlay
        .read_file(&inode, 0, &mut buf)
        .expect("underlay read");
    assert_eq!(buf, b"hi from underlay\n".to_vec());
}

#[test]
fn create_new_file_then_read_returns_new_content() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let m = Mount::open_direct(tf.path()).expect("open");

    m.overlay
        .create_file("/fresh.txt", b"fresh content".to_vec(), 0o100644);
    let content = m.read_path("/fresh.txt").expect("read fresh");
    assert_eq!(content, b"fresh content".to_vec());

    // Underlay-side lookup should fail (the file never existed there).
    assert!(m.fs.lookup_path("/fresh.txt").is_err());
}

#[test]
fn delete_file_makes_reads_fail_with_not_found() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let m = Mount::open_direct(tf.path()).expect("open");

    m.overlay.delete("/hello.txt");
    let err = m.read_path("/hello.txt").unwrap_err();
    assert_eq!(err, "not found");
}

#[test]
fn rename_moves_underlay_file_via_overlay() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let m = Mount::open_direct(tf.path()).expect("open");

    // Stage source by reading the underlay then creating-at-dest +
    // deleting-source — mirrors what the WinFsp `rename` callback does.
    let src = m.read_path("/hello.txt").expect("read src");
    m.overlay.create_file("/renamed.txt", src.clone(), 0o100644);
    m.overlay.delete("/hello.txt");

    assert_eq!(m.read_path("/renamed.txt").expect("read dst"), src);
    assert_eq!(m.read_path("/hello.txt").unwrap_err(), "not found");
}

#[test]
fn dismount_discard_resets_overlay_state() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let m = Mount::open_direct(tf.path())
        .expect("open")
        .with_dismount_policy(DismountPolicy::Discard);

    m.overlay
        .create_file("/scratch.txt", b"x".to_vec(), 0o100644);
    m.overlay.delete("/hello.txt");
    assert!(m.overlay.changes_count() > 0);
    m.apply_dismount_policy().expect("apply discard");
    assert_eq!(m.overlay.changes_count(), 0);
    // Underlay-only contents visible again.
    assert_eq!(
        m.read_path("/hello.txt").expect("post-discard read"),
        b"hi from underlay\n".to_vec()
    );
}

#[cfg(feature = "overlay-sidecar")]
#[test]
fn dismount_sidecar_writes_json_with_expected_schema() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let sidecar_dir = tempfile::tempdir().expect("tempdir");
    let sidecar_path = sidecar_dir.path().join("overlay.json");
    let m = Mount::open_direct(tf.path())
        .expect("open")
        .with_dismount_policy(DismountPolicy::Sidecar(sidecar_path.clone()));

    m.overlay
        .create_file("/new.txt", b"created".to_vec(), 0o100644);
    m.overlay.delete("/hello.txt");
    m.apply_dismount_policy().expect("apply sidecar");

    let raw = std::fs::read(&sidecar_path).expect("read sidecar");
    let json: serde_json::Value = serde_json::from_slice(&raw).expect("parse json");
    let arr = json.as_array().expect("array");
    let by_path: BTreeMap<String, &serde_json::Value> = arr
        .iter()
        .filter_map(|tuple| {
            let parts = tuple.as_array()?;
            let path = parts.get(0)?.as_str()?.to_string();
            Some((path, parts.get(1)?))
        })
        .collect();
    assert!(by_path.contains_key("/new.txt"));
    assert!(by_path.contains_key("/hello.txt"));
    // Tombstone surfaces as the bare string "Deleted" (serde's default
    // tagging for unit variants).
    let hello = by_path["/hello.txt"];
    assert!(
        hello.as_str() == Some("Deleted") || hello.is_object(),
        "expected Deleted tag, got {hello:?}"
    );
}

#[cfg(not(feature = "overlay-sidecar"))]
#[test]
fn dismount_sidecar_without_feature_errors_clearly() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let sidecar_dir = tempfile::tempdir().expect("tempdir");
    let sidecar_path = sidecar_dir.path().join("overlay.json");
    let m = Mount::open_direct(tf.path())
        .expect("open")
        .with_dismount_policy(DismountPolicy::Sidecar(sidecar_path.clone()));
    m.overlay
        .create_file("/new.txt", b"created".to_vec(), 0o100644);
    let err = m
        .apply_dismount_policy()
        .expect_err("sidecar without feature must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("overlay-sidecar"),
        "expected feature-gate hint in error, got: {msg}"
    );
    // No sidecar file should have been created.
    assert!(!sidecar_path.exists());
}

#[test]
fn dismount_rebuild_emits_new_image_with_merged_tree() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let out_dir = tempfile::tempdir().expect("tempdir");
    let out_path = out_dir.path().join("rebuilt.img");
    let m = Mount::open_direct(tf.path())
        .expect("open")
        .with_dismount_policy(DismountPolicy::Rebuild(out_path.clone()));

    // Mutations to commit:
    //  - Modify /hello.txt (truncate-then-write to mirror Explorer's
    //    overwrite-in-place sequence)
    //  - Create /fresh.txt
    //  - Delete /dir/inner.txt
    m.set_size_path("/hello.txt", 0).expect("truncate");
    m.write_path("/hello.txt", 0, b"REBUILT\n").expect("write");
    m.overlay
        .create_file("/fresh.txt", b"new\n".to_vec(), 0o100644);
    m.overlay.delete("/dir/inner.txt");

    m.apply_dismount_policy().expect("apply rebuild");
    assert!(out_path.exists(), "rebuilt image must exist on disk");

    // Open the rebuilt image and assert the merged tree is present.
    let dev = fs_core::FileDevice::open(&out_path).expect("open rebuilt");
    use std::sync::Arc;
    let dev: Arc<dyn fs_core::BlockRead> = Arc::new(dev);
    let rebuilt = Filesystem::open(dev).expect("reopen rebuilt");

    let hello = rebuilt.lookup_path("/hello.txt").expect("hello");
    let mut buf = vec![0u8; hello.size as usize];
    rebuilt.read_file(&hello, 0, &mut buf).expect("read");
    assert_eq!(buf, b"REBUILT\n".to_vec());

    let fresh = rebuilt.lookup_path("/fresh.txt").expect("fresh");
    let mut buf = vec![0u8; fresh.size as usize];
    rebuilt.read_file(&fresh, 0, &mut buf).expect("read fresh");
    assert_eq!(buf, b"new\n".to_vec());

    // Deleted entry must be gone in the rebuild.
    assert!(rebuilt.lookup_path("/dir/inner.txt").is_err());
}

#[test]
fn dismount_rebuild_round_trip_preserves_unchanged_files() {
    let img = build_underlay_image();
    let tf = write_tempimg(&img);
    let out_dir = tempfile::tempdir().expect("tempdir");
    let out_path = out_dir.path().join("rebuilt.img");
    let m = Mount::open_direct(tf.path())
        .expect("open")
        .with_dismount_policy(DismountPolicy::Rebuild(out_path.clone()));
    // No mutations at all — rebuild emits a faithful copy of the
    // underlay's regular-file + dir tree.
    m.apply_dismount_policy().expect("apply rebuild");

    let dev = fs_core::FileDevice::open(&out_path).expect("open rebuilt");
    use std::sync::Arc;
    let dev: Arc<dyn fs_core::BlockRead> = Arc::new(dev);
    let rebuilt = Filesystem::open(dev).expect("reopen rebuilt");
    let hello = rebuilt.lookup_path("/hello.txt").expect("hello");
    let mut buf = vec![0u8; hello.size as usize];
    rebuilt.read_file(&hello, 0, &mut buf).expect("read");
    assert_eq!(buf, b"hi from underlay\n".to_vec());
    let inner = rebuilt.lookup_path("/dir/inner.txt").expect("inner");
    let mut buf = vec![0u8; inner.size as usize];
    rebuilt.read_file(&inner, 0, &mut buf).expect("read inner");
    assert_eq!(buf, b"inner\n".to_vec());
}
