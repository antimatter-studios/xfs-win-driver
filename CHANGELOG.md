# Changelog

Notable changes to `xfs-win-driver`, newest first. Sections for releases before this
file existed were drawn from the commits between their tags; from here on each
release's section is written before it is tagged, and the GitHub release's
notes are that section (rust-fs-core's `release-notes`).

## [Unreleased]

### Fixed

- **The WinFsp install fails its own step when nothing was installed (#20).**
  `choco install winfsp` exits 0 when the Chocolatey feed cannot serve the
  package, and the job then failed much later in `winfsp-sys`'s build script.
  `scripts/install-winfsp.ps1` retries a feed error and refuses to finish unless
  WinFsp's headers are on disk; every workflow installs WinFsp through it.

### Changed

- **The winget manifest declares GPL-3.0-or-later**, the licence
  `Cargo.toml` and the README already state, instead of GPL-3.0 only.


### Added

- **The matrix reads a volume whose reverse-mapping tree is two levels deep.** `scripts/build-fixtures.sh` builds `xfs-rmap-deep.img` with rmapbt on and a file of 1024 one-block extents, refuses the image unless `xfs_db` reports a two-level rmap tree, and two scenarios read that file by hash, from the host and through the mount.

### Fixed

- **A transient HTTP 5xx from the chore release download no longer fails a CI job.** Every chore download in the workflows retries up to five times on any error.
- **A directory the reader cannot list is an I/O error, never an empty folder.** A directory whose contents, or one of whose children, could not be read was shown to Windows as empty or short; the listing now fails with `STATUS_IO_DEVICE_ERROR`, or `STATUS_FILE_CORRUPT_ERROR` for corrupt metadata.
- Make the XFS port build, and gate what it cannot yet test ([#1](https://github.com/antimatter-studios/xfs-win-driver/pull/1)).
- Use the ranged read, which exists ([#2](https://github.com/antimatter-studios/xfs-win-driver/pull/2)).

### Changed

- The XFS port, and what porting a win-driver actually costs.
- Real XFS filesystems, and the EROFS leftovers deleted ([#3](https://github.com/antimatter-studios/xfs-win-driver/pull/3)).
- The hooks live outside the working tree now ([#5](https://github.com/antimatter-studios/xfs-win-driver/pull/5)).
- The Windows matrix runs in CI, on the current harness, against images this repository builds ([#8](https://github.com/antimatter-studios/xfs-win-driver/pull/8)).
- A release is tested installed before it is published, and winget follows ([#9](https://github.com/antimatter-studios/xfs-win-driver/pull/9)).
- The release builds from the sibling checkouts CI uses ([#11](https://github.com/antimatter-studios/xfs-win-driver/pull/11)).
- The driver builds on rust-fs-xfs 0.12 and rust-fs-core 0.3 ([#12](https://github.com/antimatter-studios/xfs-win-driver/pull/12)).
- Wip.
- **The driver builds on rust-fs-xfs 0.12.1 and rust-fs-core 0.3.7.** Both pins are current again, and the floor of executed matrix scenarios rises from 11 to 13.


