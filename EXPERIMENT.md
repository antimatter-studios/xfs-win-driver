# Does a win-driver port by substitution?

Created 2026-09-04 by copying `erofs-win-driver` and replacing every
`erofs` with `xfs` — 435 textual references across 29 files — to test
the theory that a win-driver is boilerplate around a `rust-fs-*` reader
and a new filesystem should slot in.

**Result: partly, and the part that does not port is the part that
matters.** The substitution produced a tree that was 8 compile errors
from building and one silent factual error away from being wrong.

## What ported for free

`winfsp-fs-skeleton` already is the shared base the theory assumed. It
owns the SCM dispatcher, the disk-arrival event pump, the partition
walker, raw-device I/O with sector alignment, and drive-letter
selection — 2,243 lines, written once. A driver implements
`FsBackend`, which is **four constants and one function**:

```rust
const FS_NAME: &'static str;
const SERVICE_NAME: &'static str;
const LAUNCHER_SERVICE_CLASS: &'static str;
const FILE_EXTENSION: &'static str;
fn detect(bytes: &[u8]) -> bool;
```

That ported by rename, as did the CLI, the MSI/WiX installer, the
winget manifests and the harness config.

## What did not port, in order of how dangerous it was

### 1. The probe compiled and was silently wrong

The rename produced this, which is EROFS's superblock with XFS's name
on it:

```rust
//! XFS places its superblock at byte offset 1024 ...
//! magic number `XFS_SUPER_MAGIC_V1 = 0xE0F5E1E2`
const XFS_SUPER_OFFSET: usize = 1024;
const XFS_MAGIC: [u8; 4] = [0xE2, 0xE1, 0xF5, 0xE0];
```

XFS's superblock is at offset **0** and its magic is `XFSB`
(`0x5846_5342`), **big-endian**. Every one of those facts was wrong.
The code compiles, reads plausibly, cites a kernel header that does not
define the constant it names, and would never match an XFS volume — it
would simply report "no XFS filesystem here" forever.

Nothing about a rename pass can catch this, because there is nothing
syntactically wrong with it. It is the one part of the driver that
encodes knowledge about the filesystem itself.

`src/probe.rs` now derives the magic from
`fs_xfs::superblock::XFS_SB_MAGIC` rather than restating it, so the
driver and the reader cannot disagree, and carries a test asserting
that magic at offset 1024 does **not** match — the assertion the copied
version would have failed.

### 2. The reader APIs differ in shape, not just in name

XFS threads the **raw inode bytes** through its read calls; EROFS does
not:

| | erofs | xfs |
|---|---|---|
| constructor | `open(dev)` | `mount(dev)` / `mount_rw(dev)` |
| read a file | `read_file(&inode, offset, &mut buf) -> Result<()>` | `read_file(&inode, raw: &[u8]) -> Result<Vec<u8>>` |
| read a directory | `read_dir(&inode)` | `read_dir(&inode, raw: &[u8])` |

That extra `raw` is not a style choice — XFS inodes carry their extents
and inline data in the raw inode fork, so a reader needs the bytes
beside the parsed struct. Every call site has to obtain them via
`read_inode_raw()` and thread them through. `read_file` also inverts:
EROFS reads into a caller's buffer at an offset, XFS returns the whole
file.

That is 5 of the 8 remaining compile errors, and fixing them is a
rewrite of `mount.rs`'s read paths rather than an edit.

### 3. The overlay feature is blocked on work that does not exist

`erofs-win-driver` is read-only plus a writable in-memory overlay, and
its `rebuild` path serialises that overlay back into a fresh image via
`fs_erofs::mkfs::build_image`. **`fs_xfs` has no `mkfs` module** —
building an XFS filesystem from nothing is unfinished work (the
superblock writer landed; the AG headers, btrees, root inode and log
have not).

So an XFS driver cannot reach feature parity with the EROFS one until
mkfs.xfs exists. That is 581 lines of `overlay.rs` plus 125 references
in `mount.rs` that have no XFS counterpart yet.

## How similar are the two existing drivers, measured

| file | ext4 | erofs | identical lines | ratio |
|---|---|---|---|---|
| `mount.rs` | 1610 | 1786 | 360 | 21% |
| `probe.rs` | 57 | 52 | 25 | 46% |
| `main.rs` | 442 | 221 | 49 | 15% |

25 of ~50 function *names* in `mount.rs` are shared — those are the
WinFSP callback set the ABI demands (`open`, `close`, `read`,
`read_directory`, `get_file_info`, `get_volume_info`, `rename`,
`flush`, …). The *shape* is common because WinFSP dictates it. The
bodies are not, because each calls a different reader and ext4 is
read-write where erofs is read-only.

## State of this repo

**Builds, as of 2026-09-04.** The eight errors are fixed and CI runs on
every push. 19 unit tests pass; clippy and rustfmt are clean.

What the fixes were, since the split matters more than the count:

- **Three were the reader's shape**, exactly as predicted above. XFS
  threads the raw inode fork through `read_file` and `read_dir`, so each
  call site pairs `read_inode_raw(ino)` with the parsed inode. They now
  go through one `Mount::read_whole` helper rather than repeating it —
  and note `lookup_path` has *already* read those bytes and thrown them
  away, so every read here is two. That waste is what a handle-style API
  would remove.
- **Four were the overlay's rebuild path**, which serialises the overlay
  into a fresh image through the reader's `mkfs`. `am-fs-xfs` has none,
  so `DismountPolicy::Rebuild`, `rebuild_image` and `--scratch-rebuild`
  are **removed rather than stubbed**. A variant that always answers
  "not supported" is a promise the type cannot keep, and the flag would
  have appeared in `--help` as though it might work.
- **One was another silent rename artefact**, of the same kind as the
  probe: `cmd_info` printed `root_nid`, `meta_blkaddr` and
  `xattr_blkaddr` — EROFS fields with no XFS meaning — and compiled
  fine, because nothing is syntactically wrong with printing a field
  that means something else. It now prints XFS's own superblock.

### What is gated, and why it was not deleted

Four unit tests and the whole `tests/overlay_integration.rs` are behind
`--features xfs-mkfs-fixtures`, which nothing enables. They all need a
real XFS image, and the fixture builder inherited from the port writes
an **EROFS** superblock — XFS reads offset 0 and finds zeros. Building a
valid XFS image by hand is the same missing `mkfs` work.

They are kept because the tests are sound and only the fixture is
missing. When `mkfs.xfs` exists, enable the feature and they should
build unchanged.

### One claim in an earlier draft of this file was simply wrong

It said XFS has no ranged read, and that the WinFsp `read` callback
therefore had to fetch the whole file and slice it. `Filesystem::read_at`
is public and is exactly a ranged read; `read_file` is a thin wrapper
over it with offset 0. The port had reached for the wrapper, and the
note explained a limitation that did not exist.

Left as a lesson rather than quietly corrected, because the shape
recurs: a confident comment about *why* something must be done a certain
way is worth checking against the callee, especially in a file produced
by renaming another driver. The cost here would have been real — reading
the whole file on every callback is O(size) per read and quadratic over
a sequential scan.

**So the honest status: the driver compiles and its logic is covered
only where a fixture is not required.** Whether it mounts an XFS volume
on Windows is untested and unclaimed.

Finishing it means: thread `raw` through the read paths, drop the
overlay/rebuild feature until mkfs.xfs exists, and decide whether the
driver is read-only (matching what `fs_xfs` reliably does today) or
read-write (`mount_rw`, `write_file`, `truncate`, `set_attributes` all
exist).
