//! In-memory read-write overlay for the read-only XFS mount.
//!
//! XFS is fundamentally read-only at the format level — the on-disk
//! image carries no journal, no allocation bitmap, no free-space
//! bookkeeping. To present a writable Windows volume on top of an XFS
//! image, the WinFsp adapter consults this `Overlay` BEFORE every read
//! and stages every mutation INTO it. The underlay (`fs_xfs::Filesystem`)
//! is never touched.
//!
//! Lookup precedence:
//! 1. Overlay `Hit(entry)` → entry's contents are returned (or, for
//!    `Deleted`, NotFound is propagated).
//! 2. Overlay `Miss` → fall through to the XFS underlay.
//!
//! Path normalisation: every public method takes a `&str` and stores
//! a canonicalised key — a leading `/`, no trailing `/` (except for the
//! root itself), and `.` / `..` segments resolved before insertion.
//! This keeps overlay state consistent across WinFsp's mixed
//! `\foo\bar` / `\foo\bar\` / `\foo\.\bar` callback conventions.
//!
//! Concurrency: the overlay is wrapped in an `RwLock<BTreeMap<...>>`.
//! Reads (lookup, iter_dir) take a shared lock; writes take an exclusive
//! one. WinFsp dispatches per-file callbacks from worker threads, so
//! multiple readers hitting the same overlay-clean path proceed in
//! parallel and only writers serialise.
//!
//! License posture: GPL-3-or-later (matches the surrounding crate). All
//! code below is independently written from the design doc; no
//! adaptation of overlayfs / unionfs / fuse-overlayfs source.

use std::collections::BTreeMap;
use std::sync::RwLock;

/// Per-path overlay state. Each variant carries the full bytes / metadata
/// the read-side callbacks need so a `Hit` is self-contained — neither
/// `read` nor `get_file_info` needs to touch the underlay once a path is
/// covered by the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "overlay-sidecar",
    derive(serde::Serialize, serde::Deserialize)
)]
pub enum OverlayEntry {
    /// File created in the overlay (no underlay counterpart).
    Created {
        content: Vec<u8>,
        mode: u16,
        mtime: u64,
    },
    /// File from the underlay, modified in the overlay. We always store
    /// the FULL post-modification content so subsequent reads / writes
    /// don't have to consult the underlay again. The original underlay
    /// inode is unaffected.
    Modified {
        content: Vec<u8>,
        mode: u16,
        mtime: u64,
    },
    /// Path explicitly deleted. Reads must return
    /// `STATUS_OBJECT_NAME_NOT_FOUND` even if the underlay still has an
    /// entry at this path.
    Deleted,
    /// Directory created in the overlay (no underlay counterpart).
    CreatedDir { mode: u16, mtime: u64 },
}

impl OverlayEntry {
    /// True if the entry represents a directory (vs a regular file or
    /// tombstone). Used by `iter_dir` to decide whether to recurse.
    pub fn is_dir(&self) -> bool {
        matches!(self, OverlayEntry::CreatedDir { .. })
    }

    /// True for tombstones; reads on these paths must surface NotFound
    /// instead of falling through to the underlay.
    pub fn is_deleted(&self) -> bool {
        matches!(self, OverlayEntry::Deleted)
    }

    /// File-content accessor. Returns `None` for tombstones and
    /// directories (which carry no payload).
    pub fn content(&self) -> Option<&[u8]> {
        match self {
            OverlayEntry::Created { content, .. } | OverlayEntry::Modified { content, .. } => {
                Some(content.as_slice())
            }
            OverlayEntry::Deleted | OverlayEntry::CreatedDir { .. } => None,
        }
    }

    /// `mode` accessor (Linux-style mode bits — the high nibble is the
    /// file type). Tombstones report 0.
    pub fn mode(&self) -> u16 {
        match self {
            OverlayEntry::Created { mode, .. }
            | OverlayEntry::Modified { mode, .. }
            | OverlayEntry::CreatedDir { mode, .. } => *mode,
            OverlayEntry::Deleted => 0,
        }
    }

    /// Last-write timestamp (unix-epoch seconds). Tombstones report 0.
    pub fn mtime(&self) -> u64 {
        match self {
            OverlayEntry::Created { mtime, .. }
            | OverlayEntry::Modified { mtime, .. }
            | OverlayEntry::CreatedDir { mtime, .. } => *mtime,
            OverlayEntry::Deleted => 0,
        }
    }
}

/// Tri-state lookup result. The WinFsp adapter dispatches on this:
/// `Hit` → answer from overlay; `Deleted` → return NotFound; `Miss` →
/// consult underlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayLookup {
    /// The overlay has a positive entry at this path.
    Hit(OverlayEntry),
    /// The overlay has a tombstone at this path. Callers must NOT fall
    /// through to the underlay — the path is logically gone.
    Deleted,
    /// No overlay entry. Caller should consult the underlay.
    Miss,
}

/// Read-write overlay layered atop the read-only XFS underlay.
pub struct Overlay {
    /// Path → overlay state. Paths are stored normalised
    /// (leading `/`, no trailing `/`, `.`/`..` resolved).
    entries: RwLock<BTreeMap<String, OverlayEntry>>,
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overlay {
    /// Construct an empty overlay. All paths Miss until the first
    /// `create_file` / `write_file` / `create_dir` / `delete` call.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
        }
    }

    /// Number of paths covered by the overlay (positive entries +
    /// tombstones combined). Used by the dismount-policy code as a
    /// "is this dirty?" signal — a fresh mount that did no writes
    /// reports zero.
    pub fn changes_count(&self) -> usize {
        self.entries.read().expect("overlay lock poisoned").len()
    }

    /// Look up `path` in the overlay. The WinFsp adapter calls this
    /// before consulting the underlay on every read-side callback.
    pub fn lookup(&self, path: &str) -> OverlayLookup {
        let key = normalize_path(path);
        let map = self.entries.read().expect("overlay lock poisoned");
        match map.get(&key) {
            Some(OverlayEntry::Deleted) => OverlayLookup::Deleted,
            Some(entry) => OverlayLookup::Hit(entry.clone()),
            None => OverlayLookup::Miss,
        }
    }

    /// Stage `path` as MODIFIED with the supplied content. Used by the
    /// `write` / `set_file_size` / `overwrite` callbacks once they've
    /// resolved the new full-file bytes. If the path was previously
    /// marked Created we keep it as Created (a brand-new file's
    /// subsequent writes are still creations from the underlay's POV).
    pub fn write_file(&self, path: &str, content: Vec<u8>, mode: u16) {
        let key = normalize_path(path);
        let mut map = self.entries.write().expect("overlay lock poisoned");
        let mtime = now_seconds();
        match map.get(&key) {
            // Brand-new file: stay Created so the rebuild path emits it
            // as a fresh inode rather than carrying over an underlay one.
            Some(OverlayEntry::Created { .. }) => {
                map.insert(
                    key,
                    OverlayEntry::Created {
                        content,
                        mode,
                        mtime,
                    },
                );
            }
            _ => {
                map.insert(
                    key,
                    OverlayEntry::Modified {
                        content,
                        mode,
                        mtime,
                    },
                );
            }
        }
    }

    /// Stage `path` as a brand-new file (no underlay counterpart).
    /// Used by the `create` callback. Replaces any existing overlay
    /// entry — a path that was previously Deleted then created again
    /// becomes Created (the tombstone is lifted).
    pub fn create_file(&self, path: &str, content: Vec<u8>, mode: u16) {
        let key = normalize_path(path);
        let mut map = self.entries.write().expect("overlay lock poisoned");
        map.insert(
            key,
            OverlayEntry::Created {
                content,
                mode,
                mtime: now_seconds(),
            },
        );
    }

    /// Stage `path` as a brand-new directory.
    pub fn create_dir(&self, path: &str, mode: u16) {
        let key = normalize_path(path);
        let mut map = self.entries.write().expect("overlay lock poisoned");
        map.insert(
            key,
            OverlayEntry::CreatedDir {
                mode,
                mtime: now_seconds(),
            },
        );
    }

    /// Stage `path` as a tombstone. Future reads return NotFound even
    /// if the underlay has an entry at this path. If the entry was a
    /// purely-overlay creation, the tombstone is functionally equivalent
    /// to dropping it — but we keep the explicit `Deleted` form so the
    /// dismount-rebuild path can distinguish "user deleted underlay file"
    /// from "user never created this."
    pub fn delete(&self, path: &str) {
        let key = normalize_path(path);
        let mut map = self.entries.write().expect("overlay lock poisoned");
        map.insert(key, OverlayEntry::Deleted);
    }

    /// Move overlay state from `from` to `to`. The caller is expected to
    /// have already resolved the source content (whether from overlay or
    /// underlay) — `rename` here only touches overlay state. Returns Err
    /// if `from` is currently a tombstone (renaming a deleted thing).
    ///
    /// Spec resolution: we treat rename atomically WITHIN the overlay
    /// (one write-lock acquisition for both removals + insertions).
    /// `from` becomes Deleted (tombstone) so a subsequent lookup at the
    /// old path doesn't accidentally fall through to the underlay's
    /// still-present source. `to` adopts the source's content + mode +
    /// mtime under whichever entry kind applies.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), &'static str> {
        let from_key = normalize_path(from);
        let to_key = normalize_path(to);
        if from_key == to_key {
            return Ok(());
        }
        let mut map = self.entries.write().expect("overlay lock poisoned");
        let entry = map
            .get(&from_key)
            .cloned()
            .ok_or("rename: source has no overlay entry; caller must stage source first")?;
        if entry.is_deleted() {
            return Err("rename: source is a tombstone");
        }
        map.insert(to_key, entry);
        map.insert(from_key, OverlayEntry::Deleted);
        Ok(())
    }

    /// Return every overlay entry whose parent directory is `dir`.
    /// Used by `read_directory` to merge overlay-only Creates into the
    /// underlay listing AND to identify Deleted children to filter out.
    /// Returns the entries with their LEAF names (not full paths) so the
    /// caller can drop them straight into the WinFsp `DirInfo` buffer.
    pub fn iter_dir(&self, dir: &str) -> Vec<(String, OverlayEntry)> {
        let dir_key = normalize_path(dir);
        let map = self.entries.read().expect("overlay lock poisoned");
        let prefix = if dir_key == "/" {
            String::from("/")
        } else {
            format!("{dir_key}/")
        };
        let mut out = Vec::new();
        for (k, v) in map.iter() {
            if !k.starts_with(&prefix) {
                continue;
            }
            let leaf = &k[prefix.len()..];
            if leaf.is_empty() || leaf.contains('/') {
                // Either the dir itself, or a deeper grandchild — skip.
                continue;
            }
            out.push((leaf.to_string(), v.clone()));
        }
        out
    }

    /// Drop every overlay entry. Used by the `Discard` dismount policy
    /// and by tests that re-use a single Overlay across scenarios.
    pub fn clear(&self) {
        self.entries.write().expect("overlay lock poisoned").clear();
    }

    /// Take a snapshot of every overlay entry as `(path, entry)` pairs,
    /// sorted by path. Used by the dismount-sidecar policy to serialise
    /// overlay state and by the rebuild path to walk the merged tree.
    pub fn snapshot(&self) -> Vec<(String, OverlayEntry)> {
        let map = self.entries.read().expect("overlay lock poisoned");
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// Normalise a path key: ensure leading `/`, no trailing `/`, all `.`
/// segments dropped, all `..` segments resolved against the preceding
/// segment. Backslashes are translated to forward slashes so WinFsp's
/// `\foo\bar` paths land on the same key as the unix-style `/foo/bar`.
///
/// `..` that would escape the root (e.g. `/a/../../b`) is clamped — the
/// extra `..` is silently dropped, mirroring Windows' typical
/// "reach above the volume root" behaviour. We never want a key with a
/// leading `..` slipping into the overlay map.
pub fn normalize_path(input: &str) -> String {
    let unified = input.replace('\\', "/");
    let mut stack: Vec<&str> = Vec::new();
    for segment in unified.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            stack.pop();
            continue;
        }
        stack.push(segment);
    }
    if stack.is_empty() {
        return String::from("/");
    }
    let mut out = String::with_capacity(unified.len());
    for s in stack {
        out.push('/');
        out.push_str(s);
    }
    out
}

/// Current wall-clock time as unix-epoch seconds. Falls back to 0 if
/// the system clock is set before 1970 — exotic but possible on a
/// freshly-flashed embedded device.
fn now_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Unit tests for the overlay's path-keyed state machine. The
    //! underlay is faked via direct `OverlayLookup` assertions — these
    //! tests don't touch `fs_xfs::Filesystem` at all. End-to-end
    //! merging with a real XFS underlay lives in
    //! `tests/overlay_integration.rs`.
    use super::*;

    #[test]
    fn created_file_lookup_returns_content() {
        let ov = Overlay::new();
        ov.create_file("/hello.txt", b"hi\n".to_vec(), 0o100644);
        match ov.lookup("/hello.txt") {
            OverlayLookup::Hit(OverlayEntry::Created { content, mode, .. }) => {
                assert_eq!(content, b"hi\n".to_vec());
                assert_eq!(mode, 0o100644);
            }
            other => panic!("expected Hit(Created), got {other:?}"),
        }
    }

    #[test]
    fn modified_file_overrides_underlay() {
        // We model the underlay as an external decision — for this test
        // we just need to confirm: once an overlay Modified entry is in
        // place, lookup returns it and a hypothetical underlay-fallback
        // would never run.
        let ov = Overlay::new();
        // Simulate "underlay had /a; user wrote new content".
        ov.write_file("/a", b"new".to_vec(), 0o100644);
        match ov.lookup("/a") {
            OverlayLookup::Hit(OverlayEntry::Modified { content, .. }) => {
                assert_eq!(content, b"new".to_vec());
            }
            other => panic!("expected Hit(Modified), got {other:?}"),
        }
    }

    #[test]
    fn deleted_path_hides_underlay() {
        let ov = Overlay::new();
        ov.delete("/gone");
        assert!(matches!(ov.lookup("/gone"), OverlayLookup::Deleted));
    }

    #[test]
    fn iter_dir_merges_overlay_and_underlay() {
        // The merging itself happens in mount.rs; here we assert the
        // overlay reports the right set of children for a given dir.
        let ov = Overlay::new();
        ov.create_file("/dir/new.txt", b"x".to_vec(), 0o100644);
        ov.create_dir("/dir/sub", 0o040755);
        ov.create_file("/dir/sub/inner.txt", b"y".to_vec(), 0o100644);
        let mut entries = ov.iter_dir("/dir");
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        // Should see direct children only — no recursion into sub.
        assert_eq!(names, vec!["new.txt", "sub"]);
    }

    #[test]
    fn iter_dir_hides_deleted() {
        let ov = Overlay::new();
        ov.delete("/dir/old.txt");
        let entries = ov.iter_dir("/dir");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].1.is_deleted());
        // The mount.rs caller is responsible for filtering tombstoned
        // names from underlay listings — here we just confirm the
        // overlay surfaces them so the caller has the data it needs.
    }

    #[test]
    fn rename_moves_overlay_state() {
        let ov = Overlay::new();
        ov.create_file("/from.txt", b"hello".to_vec(), 0o100644);
        ov.rename("/from.txt", "/to.txt").expect("rename");
        // Old path: tombstoned.
        assert!(matches!(ov.lookup("/from.txt"), OverlayLookup::Deleted));
        // New path: content carried over.
        match ov.lookup("/to.txt") {
            OverlayLookup::Hit(OverlayEntry::Created { content, .. }) => {
                assert_eq!(content, b"hello".to_vec());
            }
            other => panic!("expected Hit(Created) at /to.txt, got {other:?}"),
        }
    }

    #[test]
    fn rename_of_missing_source_errors() {
        let ov = Overlay::new();
        let err = ov.rename("/nope", "/other");
        assert!(err.is_err(), "rename on overlay-absent source must error");
    }

    #[test]
    fn rename_same_path_is_noop() {
        let ov = Overlay::new();
        ov.create_file("/a", b"x".to_vec(), 0o100644);
        ov.rename("/a", "/a").expect("self-rename");
        match ov.lookup("/a") {
            OverlayLookup::Hit(OverlayEntry::Created { .. }) => {}
            other => panic!("expected /a to still be Created, got {other:?}"),
        }
    }

    #[test]
    fn path_normalization_dot_dotdot_handled() {
        // Various WinFsp-callback-shape inputs that should all canonicalise
        // to `/foo/bar`.
        assert_eq!(normalize_path("/foo/bar"), "/foo/bar");
        assert_eq!(normalize_path("/foo//bar"), "/foo/bar");
        assert_eq!(normalize_path("/foo/./bar"), "/foo/bar");
        assert_eq!(normalize_path("/foo/baz/../bar"), "/foo/bar");
        assert_eq!(normalize_path("\\foo\\bar"), "/foo/bar");
        assert_eq!(normalize_path("foo/bar"), "/foo/bar");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("/.."), "/");
        // Reaching above root is clamped, not propagated.
        assert_eq!(normalize_path("/../../etc"), "/etc");
    }

    #[test]
    fn nested_dir_create_then_lookup_subfile() {
        let ov = Overlay::new();
        ov.create_dir("/proj", 0o040755);
        ov.create_dir("/proj/src", 0o040755);
        ov.create_file("/proj/src/main.rs", b"fn main(){}".to_vec(), 0o100644);
        match ov.lookup("/proj/src/main.rs") {
            OverlayLookup::Hit(OverlayEntry::Created { content, .. }) => {
                assert_eq!(content, b"fn main(){}".to_vec());
            }
            other => panic!("unexpected lookup: {other:?}"),
        }
        // Both intermediate dirs report as overlay-Created dirs.
        assert!(matches!(
            ov.lookup("/proj"),
            OverlayLookup::Hit(OverlayEntry::CreatedDir { .. })
        ));
        assert!(matches!(
            ov.lookup("/proj/src"),
            OverlayLookup::Hit(OverlayEntry::CreatedDir { .. })
        ));
        // /proj/src.iter_dir lists the file we just created.
        let listed = ov.iter_dir("/proj/src");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "main.rs");
    }

    #[test]
    fn changes_count_tracks_writes_and_deletes() {
        let ov = Overlay::new();
        assert_eq!(ov.changes_count(), 0);
        ov.create_file("/a", b"x".to_vec(), 0o100644);
        assert_eq!(ov.changes_count(), 1);
        ov.delete("/a");
        // delete overwrites the same key — count stays at 1 (Deleted
        // tombstone replaced the Created entry).
        assert_eq!(ov.changes_count(), 1);
        ov.delete("/b");
        assert_eq!(ov.changes_count(), 2);
    }

    #[test]
    fn clear_resets_all_state() {
        let ov = Overlay::new();
        ov.create_file("/a", b"x".to_vec(), 0o100644);
        ov.delete("/b");
        ov.create_dir("/c", 0o040755);
        assert_eq!(ov.changes_count(), 3);
        ov.clear();
        assert_eq!(ov.changes_count(), 0);
        assert!(matches!(ov.lookup("/a"), OverlayLookup::Miss));
    }

    #[test]
    fn snapshot_returns_sorted_entries() {
        let ov = Overlay::new();
        ov.create_file("/b", b"2".to_vec(), 0o100644);
        ov.create_file("/a", b"1".to_vec(), 0o100644);
        ov.create_file("/c", b"3".to_vec(), 0o100644);
        let snap = ov.snapshot();
        let keys: Vec<&str> = snap.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["/a", "/b", "/c"]);
    }

    #[test]
    fn overlay_entry_helpers() {
        let c = OverlayEntry::Created {
            content: vec![1, 2, 3],
            mode: 0o100644,
            mtime: 42,
        };
        assert_eq!(c.content(), Some(&[1u8, 2, 3][..]));
        assert_eq!(c.mode(), 0o100644);
        assert_eq!(c.mtime(), 42);
        assert!(!c.is_dir());
        assert!(!c.is_deleted());

        let d = OverlayEntry::CreatedDir {
            mode: 0o040755,
            mtime: 7,
        };
        assert!(d.is_dir());
        assert_eq!(d.content(), None);

        let t = OverlayEntry::Deleted;
        assert!(t.is_deleted());
        assert_eq!(t.content(), None);
        assert_eq!(t.mode(), 0);
        assert_eq!(t.mtime(), 0);
    }
}
