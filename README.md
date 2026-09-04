# xfs-win-driver

## Overview

Windows-first userspace tooling for **XFS** (Enhanced Read-Only File System) volumes, built on the [rust-fs-xfs](https://github.com/antimatter-studios/rust-fs-xfs) library (crate `am-fs-xfs`) and the [WinFsp](https://winfsp.dev/) FUSE-equivalent for Windows.

XFS is the on-disk format used for `system.img`, `vendor.img`, `product.img` on Android 10+, and increasingly for ChromeOS containers and embedded immutable-OS images. This driver lets Windows users browse those images natively as drive letters.

Scope:

1. **Auto-mount service** — plug an XFS-bearing SD card or USB stick into Windows and the `XfsWatcher` service mounts it on a free drive letter in the active console session. Detach and it unmounts cleanly.
2. **Right-click verb** — "Mount as xfs" on `.img` files in Explorer for offline disk images.
3. **CLI browser** — open an XFS image or raw device; list/read files without mounting. Cross-platform (macOS/Linux work too — handy for testing).
4. **Read-write overlay** — XFS is read-only by format, but the driver fakes a writable volume via an in-memory overlay layer. Stage edits, then either discard them, archive them to a JSON sidecar, or commit them by rebuilding a new XFS image. See [Read/write semantics](#readwrite-semantics) below.
5. **Setup.exe** — bundles WinFsp via a Burn bootstrapper, so end users only run one installer.

The library lives in the [rust-fs-xfs](https://github.com/antimatter-studios/rust-fs-xfs) project (crate `am-fs-xfs`), path-depended at `../rust-fs-xfs/`; this crate is the distribution unit.

The Windows-driver scaffolding (SCM service, disk-arrival watcher, WinFsp.Launcher integration, partition-table walker, raw-device I/O, installer + CI templates) lives in [winfsp-fs-skeleton](https://github.com/antimatter-studios/winfsp-fs-skeleton). xfs-win-driver is the **second consumer** of that skeleton (after `ext4-win-driver`). See [The skeleton split](#the-skeleton-split) below for the boundary.

## Status

Currently in development; first release pending.

- [x] CLI: `info`, `ls`, `cat`, `watch`, `service`, `mount`
- [x] MBR/GPT partition parsing (via the skeleton's `partition` module)
- [x] `--part N` mounts a partition slice via WinFsp's `FileSystemContext`
- [x] Win32 raw-device support (`\\.\X:`, `\\.\PhysicalDriveN`, `\\?\STORAGE#Disk#...`) with sector-aligned reads for 4Kn / advanced-format drives
- [x] **WinFsp read-only mount** — full `FileSystemContext` impl reading every XFS feature the underlying library supports (LZ4/LZMA/DEFLATE compression, compacted-2B index, ztailpacking, fragments, big_pcluster, xattrs, ACLs, symlinks)
- [x] **WinFsp read-write mount** via in-memory overlay layer — `create`/`write`/`set_file_size`/`overwrite`/`rename`/`set_delete` all wired; reads consult the overlay first then fall through to the XFS underlay
- [x] Three dismount policies: `--scratch-discard` (default), `--scratch-sidecar PATH` (JSON dump of changes), `--scratch-rebuild PATH` (re-mkfs the merged tree)
- [x] **`XfsWatcher` SCM service** — subscribes to disk-class arrivals (`GUID_DEVINTERFACE_DISK`), walks the partition table, probes each partition for the XFS superblock magic at offset 1024, and asks WinFsp.Launcher to spawn a per-partition mount in the active console session
- [x] **Right-click .img → Mount as xfs** verb registered under `HKCR\SystemFileAssociations\.img`
- [x] **MSI + Burn bundle** that auto-installs WinFsp if missing
- [x] **x64 + arm64 Setup.exe** built per release tag by GH Actions (`.github/workflows/release.yml`)
- [x] 31 tests passing (8 backend + 14 overlay unit + 9 integration)

Not yet shipped:

- [ ] First versioned release (`v0.1.0`)
- [ ] winget submission (manifests staged at [`winget/`](./winget/))
- [ ] Runtime testing on a Windows host with WinFsp installed (development happens on macOS; the Windows mount path compiles cleanly cross-target but hasn't been smoke-tested on real Windows yet — contributors with Windows 11 + WinFsp welcomed)

Out of scope (intentionally):

- **Mutating XFS images in place.** XFS is read-only by format spec — there is no journal, no allocator, no rewrite path. The "writable mount" is a UI fiction backed by an in-memory overlay; persistence options are documented below.
- **Code-signing certificate** — the per-year cost isn't justified for an unsponsored side project. Setup.exe will continue to trip SmartScreen on first download until that changes.
- **Verified-boot / dm-verity hash trees** — adjacent concern, sits above the XFS layer; use `verity` tools alongside.

## Install (end users)

Once released, download `xfs-win-driver-<ver>-<arch>-Setup.exe` (`arch` is `x64` or `arm64`) from [Releases](https://github.com/antimatter-studios/xfs-win-driver/releases), run it, accept the GPL-3 prompt. The installer:

- chains the WinFsp MSI if WinFsp isn't already installed,
- drops `xfs.exe` and `Mount-Xfs.ps1` into `C:\Program Files\xfs-win-driver\`,
- registers `XfsWatcher` as an automatic Windows service and starts it,
- registers the `xfs-mount` service-class with WinFsp.Launcher,
- registers the "Mount as xfs" right-click verb on `.img` files.

`Setup.exe /quiet` works for unattended installs. SCCM/Intune deploys should push the bare MSI separately, after pushing WinFsp.

## Usage

Plug in an XFS-bearing SD card or USB stick: nothing else needed. The service picks it up, finds the XFS partition(s), and mounts each on a free drive letter (E: upward).

CLI (works on any host — macOS/Linux for browsing; Windows for browsing + mounting):

```
xfs info  <image>                        # superblock fields, block size, root NID
xfs ls    <image> [path]                 # directory listing (default: /)
xfs cat   <image> <path>                 # print file contents to stdout
```

Manual WinFsp mount (Windows + `mount` feature):

```
xfs mount <image> --drive X:                                   # read-write overlay (default)
xfs mount <image> --drive X: --ro                              # read-only (rejects writes)
xfs mount <whole-disk.img> --drive X: --part 1                 # mount partition 1
xfs mount <image> --drive X: --scratch-sidecar changes.json    # archive writes on dismount
xfs mount <image> --drive X: --scratch-rebuild new.img         # commit writes by re-mkfs
```

Then browse `X:` in Explorer, or `Get-ChildItem X:\`, etc. Ctrl-C to unmount.

Watch mode (foreground variant of the service, useful for dev / debugging):

```
xfs watch                                  # foreground; logs each arrival
```

### Read/write semantics

XFS is **read-only by format design** — the on-disk format has no journal, no allocator, and no rewrite path. The driver presents a writable volume to Windows by capturing every write in an **in-memory overlay layer**; reads consult the overlay first and fall through to the XFS underlay only on miss.

Three dismount policies decide what happens to the staged writes:

| Flag | What it does | When to use |
|---|---|---|
| `--scratch-discard` (default) | Overlay is dropped. The image on disk is unchanged. | Read-only browsing of an image you don't want to modify |
| `--scratch-sidecar <PATH>` | Overlay state serialized to a JSON file at `<PATH>` on dismount | Audit what was edited; replay later |
| `--scratch-rebuild <PATH>` | Walk the merged tree (overlay + underlay), emit a new XFS image at `<PATH>` via the bundled `mkfs_xfs` library function | Commit changes to a new image |
| `--ro` | Reject all writes at the WinFsp callback layer with `STATUS_MEDIA_WRITE_PROTECTED` | Force read-only even on the "default" R/W mount |

**Caveat: the overlay is in-memory.** Large writes can OOM. There is no streaming-to-disk overflow path. If you intend to write multi-GB into a mounted volume, use `--scratch-rebuild` and ensure host RAM is proportional to the changes.

The right-click verb invokes [`installer/Mount-Xfs.ps1`](./installer/Mount-Xfs.ps1) which takes a `-ReadOnly` switch. To force the verb itself read-only, edit `HKCR\SystemFileAssociations\.img\shell\MountAsXfs\command` to add `-ReadOnly` to the script invocation.

To force every auto-mount through `XfsWatcher` to be read-only, edit `HKLM\SOFTWARE\WOW6432Node\WinFsp\Services\xfs-mount\CommandLine` (32-bit registry view) and append ` --ro` to the default `mount %2 --drive %1 --part %3` template.

### What the driver can read

Every XFS feature emitted by `mkfs.xfs` 1.9 + AOSP build systems:
- Compact (32-byte) and extended (64-byte) inodes
- `FLAT_PLAIN`, `FLAT_INLINE` (tail-packed), `ChunkBased` (with sparse holes), `Compression` layouts
- Codecs: **LZ4**, **LZMA** (raw LZMA1), **DEFLATE** (raw, no zlib wrapper)
- Index formats: legacy/uncompacted (8-byte entries) and compacted-2B (bitstream)
- Optimizations: `BIG_PCLUSTER_1/2`, `FRAGMENT_PCLUSTER` (cross-file packed-tail), `INTERLACED_PCLUSTER` (rotate-and-paste PLAIN), `INLINE_PCLUSTER` (ztailpacking), `HEAD2` separate-codec dispatch, `COMPR_CFGS` blob (LZMA dict_size etc.)
- Inline xattrs, shared (block-area) xattrs, custom xattr prefix dictionary
- POSIX ACLs (access + default)
- Symbolic links (with loop protection at MAXSYMLINKS=40), special files (chrdev, blkdev, fifo, socket), hardlinks
- Multi-device images (primary + extra-devices via `device_id` routing)

See the [rust-fs-xfs README](https://github.com/antimatter-studios/rust-fs-xfs/blob/main/README.md) for the full feature matrix and limitations.

### Performance

The library uses an LRU cache (default 256 entries ≈ 64 MiB) of decompressed pclusters; sequential reads of a multi-pcluster compressed file see roughly 8× speedup from cache hits. Cache is per-`Filesystem` instance, so each mount has its own. Memory-constrained hosts can opt out via the library API.

## The skeleton split

The platform plumbing was extracted into [winfsp-fs-skeleton](https://github.com/antimatter-studios/winfsp-fs-skeleton) so the same scaffolding can host filesystem drivers (ext4, ntfs, ...) without copy-paste. The boundary:

| Lives in skeleton (reusable) | Lives here (XFS-specific) |
|---|---|
| `service::run<B>` — SCM dispatcher + WinFsp.Launcher | [`src/main.rs`](./src/main.rs) — `XfsBackend` impl (4 const + 1 fn), CLI dispatch |
| `watch::run<B>` — foreground variant | [`src/mount.rs`](./src/mount.rs) — WinFsp `FileSystemContext` impl, am-fs-xfs callbacks |
| `partition` — MBR/GPT parsing | [`src/probe.rs`](./src/probe.rs) — `is_xfs` magic-byte predicate |
| `device` — `BlockSource` + sector-aligned `FileSource` | [`src/overlay.rs`](./src/overlay.rs) — in-memory R/W overlay layer |
| `probe` — drive-letter selection, `GUID_DEVINTERFACE_DISK`, `DEV_BROADCAST_DEVICEINTERFACE_W` parsing | |
| `templates/installer/` — WiX MSI + Burn shapes | [`installer/`](./installer/) — XFS-customised copies of the templates |
| `templates/release.yml` — GH Actions x64 + arm64 build matrix | [`.github/workflows/release.yml`](./.github/workflows/release.yml) |
| `templates/winget/` — manifest skeleton | [`winget/`](./winget/) |

The public seam is one trait + four constants:

```rust
use winfsp_fs_skeleton::FsBackend;

struct XfsBackend;
impl FsBackend for XfsBackend {
    const FS_NAME: &'static str = "xfs";
    const SERVICE_NAME: &'static str = "XfsWatcher";
    const LAUNCHER_SERVICE_CLASS: &'static str = "xfs-mount";
    const FILE_EXTENSION: &'static str = "img";
    fn detect(bytes: &[u8]) -> bool { probe::is_xfs(bytes) }
}
```

`Cmd::Watch` and `Cmd::Service` in [`src/main.rs`](./src/main.rs) just hand off:

```rust
Cmd::Watch   => winfsp_fs_skeleton::watch::run::<XfsBackend>(),
Cmd::Service => winfsp_fs_skeleton::service::run::<XfsBackend>(),
```

## Build

CLI only (any platform):

```
cargo build --release
```

WinFsp mount + auto-mount service (Windows only):

```
cargo build --release --features mount
```

The `mount` feature pulls in the `winfsp` / `winfsp-sys` / `windows` / `widestring` crates from the path-depended fork at `../winfsp-rs/`. On non-Windows targets the WinFsp adapter is `cfg`-gated to nothing, so the same source tree builds cleanly on macOS/Linux for unit testing the cross-platform overlay layer.

The release profile statically links the C runtime via [`.cargo/config.toml`](./.cargo/config.toml) so the binary has no `libunwind.dll` import — required for the SCM session-0 launch path where the LocalSystem PATH doesn't reach the LLVM-MinGW runtime dir. `panic = "abort"` is set in `Cargo.toml` for the same release profile.

### Optional sidecar feature

```
cargo build --release --features mount,overlay-sidecar
```

The `overlay-sidecar` feature pulls `serde` + `serde_json` and enables `--scratch-sidecar PATH` to serialize the overlay state on dismount. Without the feature, `--scratch-sidecar` errors clearly. Default is **off** to keep the binary small.

### WinFsp build prerequisites

- **WinFsp 2.1+** installed on the build/run machine ([winfsp.dev](https://winfsp.dev/) → MSI, or `winget install WinFsp.WinFsp`).
- A forked [winfsp-rs](https://github.com/antimatter-studios/winfsp-rs) is path-depended at `../winfsp-rs/` on the `gnullvm-support` branch (the upstream PR is pending). The fork also requires:
  - `LLVM` for `libclang.dll` (`winget install LLVM.LLVM`)
  - LLVM-MinGW (`winget install MartinStorsjo.LLVM-MinGW.UCRT`)
- `LIBCLANG_PATH=C:\Program Files\LLVM\bin` so bindgen can find `libclang.dll`.
- [winfsp-fs-skeleton](https://github.com/antimatter-studios/winfsp-fs-skeleton) is path-depended at `../winfsp-fs-skeleton/`; pure Rust, no extra toolchain requirements.

### Building the installer

After `cargo build --release --features mount` finishes:

```powershell
installer\build.ps1 -ExePath target\release\xfs.exe -Arch arm64
```

Produces `dist\xfs-win-driver-<ver>-arm64.msi` and `dist\xfs-win-driver-<ver>-arm64-Setup.exe`. The script PE-sniffs `-ExePath` to catch arch mismatches before WiX does.

## Testing

```
cargo test                                            # 31 tests on macOS/Linux/Windows
cargo test --features mount,overlay-sidecar           # adds dismount-sidecar test path
```

Tests run cross-platform — the WinFsp adapter is cfg-gated, so the overlay layer + read paths are exercised on macOS via cross-platform helpers (`Mount::read_path` / `write_path` / `set_size_path`) that mirror the WinFsp callback semantics.

For end-to-end runtime testing on a Windows host:

```
cargo test --features mount -- --ignored full_winfsp_mount_smoke
```

This requires WinFsp installed and runs only on Windows. (The `--ignored` test count above is currently 1.)

## Known limitations

- **XFS is read-only by format**. The "writable mount" is an in-memory overlay; persistence requires explicit `--scratch-sidecar` or `--scratch-rebuild` on dismount.
- **Overlay is in-memory**. Large writes (multi-GB) can OOM. Use `--scratch-rebuild` and provision RAM accordingly.
- **No multi-device WRITER**. The `am-fs-xfs` library reads multi-device images correctly, but the bundled `mkfs_xfs` doesn't emit them yet (upstream `mkfs.xfs --blobdev` is broken in 1.9 — we're waiting on the fix to validate against an oracle).
- **Right-click verb caveat**: opening an `.img` containing multiple partitions via "Mount as xfs" mounts only the first detected XFS partition. Use the CLI with `--part N` for partition selection.
- **Windows runtime testing pending**: development happens on macOS where the cross-platform paths are exercised. The actual WinFsp-on-Windows mount path is compile-clean but hasn't been smoke-tested on a real Windows host yet. Contributors with a WinFsp-equipped Windows 11 box welcomed.

## License

GPL-3.0-or-later — inherited from the WinFsp Rust bindings link line. The CLI subcommands that don't link winfsp (`info`, `ls`, `cat`, `watch`) work cross-platform and could be relicensed if split out, but the single-license declaration keeps the distribution unit simple.

The underlying [rust-fs-xfs](https://github.com/antimatter-studios/rust-fs-xfs) library (crate `am-fs-xfs`) is **MIT** — one-way compatible (MIT flows cleanly into GPL-3 distributions). All transitive dependencies are permissive (MIT / Apache-2 / BSD / Zlib / 0BSD); no GPL/LGPL pulled in. A pre-distribution IP audit confirms the cleanroom posture.

External tools (`mkfs.xfs`, `fsck.xfs`, `dump.xfs`) are invoked at arm's length via subprocess from `#[ignore]`-gated integration tests in the library only — never linked, never source-copied.

## Acknowledgements

XFS originally developed by Huawei (~2018, upstreamed Linux 5.4). Format documentation maintained by the XFS upstream project at [xfs.docs.kernel.org](https://xfs.docs.kernel.org/). WinFsp by [Bill Zissimopoulos](https://winfsp.dev/). Sister project [ext4-win-driver](https://github.com/antimatter-studios/ext4-win-driver) was the first consumer of the `winfsp-fs-skeleton` and inspired this driver's structure.
