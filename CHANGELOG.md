# Changelog

Notable changes to `xfs-win-driver`, newest first. Sections for releases before this
file existed were drawn from the commits between their tags; from here on each
release's section is written before it is tagged, and the GitHub release's
notes are that section (rust-fs-core's `release-notes`).

## [Unreleased]

### Fixed

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


