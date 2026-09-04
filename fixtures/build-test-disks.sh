#!/usr/bin/env bash
# build-test-disks.sh -- materialise XFS test images referenced by
# test-matrix.json into vendor/rust-fs-xfs/test-disks/.
#
# Test images are build artefacts (not tracked in git). This script
# synthesises a minimal source tree, then invokes the mkfs_xfs binary
# from the path-dep'd vendor/rust-fs-xfs crate to produce one or
# more XFS images.
#
# Usage:
#   bash fixtures/build-test-disks.sh
#
# Outputs:
#   ../rust-fs-xfs/test-disks/xfs-basic.img   — ~minimal tree
#                                                    (test.txt, subdir/nested.txt)
#
# Re-run after any change to fixture file content; the harness
# scenarios pin sha256 + size of these files so drift is caught.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
disks_dir="${repo_root}/../rust-fs-xfs/test-disks"

# Build mkfs_xfs from the vendored sibling project.
mkfs_root="${repo_root}/vendor/rust-fs-xfs"
mkfs_bin="${mkfs_root}/target/release/mkfs_xfs"

echo "[build] cargo build --release --bin mkfs_xfs"
cd "${mkfs_root}"
cargo build --release --bin mkfs_xfs --quiet

if [[ ! -x "${mkfs_bin}" ]]; then
    echo "build-test-disks.sh: mkfs_xfs not built at ${mkfs_bin}" >&2
    exit 1
fi

mkdir -p "${disks_dir}"

build_basic() {
    local out="${disks_dir}/xfs-basic.img"
    local src
    src=$(mktemp -d -t xfs-basic.XXXXXX)
    trap 'rm -rf "${src}"' RETURN

    # Stable content — sha256 + size are pinned in test-matrix.json.
    printf 'hello from xfs\n' > "${src}/test.txt"
    mkdir -p "${src}/subdir"
    printf 'nested\n' > "${src}/subdir/nested.txt"

    echo "[mkfs] ${out}"
    "${mkfs_bin}" "${out}" "${src}"
}

build_basic

echo "done. images in ${disks_dir}/"
ls -la "${disks_dir}"
