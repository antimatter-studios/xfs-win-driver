#!/usr/bin/env bash
#
# Build the XFS images the tests read.
#
# WHY THIS EXISTS. The tests used to run against a hand-built image
# copied from erofs-win-driver, which wrote an EROFS superblock -- so
# XFS read offset 0, found zeros, and every test that needed a mounted
# filesystem failed. Gating them made the failures go away without
# making the tests true.
#
# These are real filesystems, made by mkfs.xfs and populated by the
# kernel, so a test that reads one is checking this driver against what
# XFS actually writes rather than against our own idea of it.
#
# WHAT IT NEEDS. mkfs.xfs (xfsprogs) to create, and root to mount and
# populate. That means Linux: CI runs it on ubuntu-latest, and a Linux
# dev box or VM runs it locally. On macOS there is no mkfs.xfs, so the
# tests skip and say so -- see tests/common/mod.rs.
#
# THE CONTENT IS DELIBERATE. Every file here exists because a test
# asserts something specific about it. Adding a case means adding a file
# here and asserting on it there; a test that just walks "whatever mkfs
# left" proves very little.
#
# Usage:  scripts/build-fixtures.sh [output-dir]     (default: .fixtures)
set -euo pipefail

OUT=${1:-.fixtures}
IMG="$OUT/xfs-content.img"
MNT=$(mktemp -d)

command -v mkfs.xfs >/dev/null || {
  echo "mkfs.xfs not found. Install xfsprogs (Linux); this cannot run on macOS." >&2
  exit 1
}
[ "$(id -u)" = 0 ] || { echo "must run as root: the image has to be mounted to populate it" >&2; exit 1; }

mkdir -p "$OUT"
rm -f "$IMG"

# 300 MiB: comfortably over mkfs.xfs's ~16 MiB floor, small enough to
# build in a second and to hold in a runner's disk without thought.
truncate -s 300M "$IMG"
mkfs.xfs -q -L DJTEST "$IMG"

mount -o loop "$IMG" "$MNT"
trap 'umount "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT

# --- the deliberate content ------------------------------------------
# Each line is asserted somewhere in tests/. Keep the two in step.

# A short file whose exact bytes a test compares. No trailing newline,
# so the length is exactly what it looks like.
printf 'hello xfs' > "$MNT/small.txt"

# Empty: is_empty(), a zero-length read, and reading at offset 0 of
# nothing must all behave rather than error.
: > "$MNT/empty.txt"

# Big enough to span several extents and several blocks, with a
# POSITION-DEPENDENT pattern so a ranged read that returns the right
# NUMBER of bytes from the WRONG OFFSET still fails. A constant fill
# would hide exactly that bug.
python3 - "$MNT/pattern.bin" <<'PY'
import sys
size = 3 * 1024 * 1024
with open(sys.argv[1], "wb") as f:
    # Each 8-byte group is its own offset, little-endian.
    f.write(b"".join((i).to_bytes(8, "little") for i in range(0, size // 8)))
PY

# Nested directories, so a walk has somewhere to go and path joining is
# exercised beyond one level.
mkdir -p "$MNT/dir/nested/deep"
printf 'level three' > "$MNT/dir/nested/deep/leaf.txt"
printf 'level one' > "$MNT/dir/one.txt"

# Enough entries to push the directory past short form into block form,
# which is a different on-disk representation and a different code path.
mkdir -p "$MNT/manyentries"
for i in $(seq 1 200); do printf 'e%s' "$i" > "$MNT/manyentries/entry-$i.txt"; done

# A symlink, and one that dangles: reading the target must work without
# resolving it, and a dangling target is a normal thing on disk rather
# than an error.
ln -s /small.txt "$MNT/link-to-small"
ln -s /nowhere-at-all "$MNT/dangling-link"

# A file whose name is not ASCII. XFS stores names as bytes and does not
# require valid UTF-8, so the driver must not assume it.
printf 'unicode name' > "$MNT/naïve-café.txt"

sync
umount "$MNT"
trap - EXIT
rmdir "$MNT"

echo "built $IMG"
ls -la "$IMG"

# --- xfs-rmap-deep.img: trees more than one block deep ----------------
#
# A reverse-mapping B+tree is an interval tree: a node entry carries the
# lowest AND the highest key beneath it, so its pointer array starts twice
# as far into the block as a one-key-per-entry reading assumes. A reader
# that gets that wrong takes a pointer from the middle of the key array,
# and on a real volume it came back as block zero -- the superblock. It
# cannot happen while the tree is one block deep, which is why
# xfs-content.img never showed it: it needs more mappings in one group
# than a 4 KiB leaf holds (168).
#
# So this image has one file of 2048 blocks with every odd block punched
# out: 1024 one-block extents, every one its own reverse mapping and its
# own block-map record. That makes the group's rmap tree two levels deep,
# and the file's own block map a B+tree with several leaves under the
# inode, which is the read this driver does on every cat of it.
#
# The content is position-dependent like pattern.bin, and a punched block
# reads as zeros: a read from the wrong extent fails the hash.
DEEP="$OUT/xfs-rmap-deep.img"
rm -f "$DEEP"
truncate -s 128M "$DEEP"
# rmapbt is named rather than left to mkfs's default, which has changed
# across xfsprogs releases: this image exists to have the tree.
mkfs.xfs -q -L DJDEEP -m rmapbt=1 "$DEEP"

MNT=$(mktemp -d)
mount -o loop "$DEEP" "$MNT"
trap 'umount "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT

mkdir -p "$MNT/deep"
python3 - "$MNT/deep/fragmented.bin" <<'PY'
import ctypes, os, sys
BS, BLOCKS = 4096, 2048
fd = os.open(sys.argv[1], os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o644)
for b in range(BLOCKS):
    os.write(fd, b"".join(i.to_bytes(8, "little") for i in range(b * BS // 8, (b + 1) * BS // 8)))
# Allocated before any hole is punched, so the holes split real extents
# rather than shaping a delayed allocation.
os.fsync(fd)
libc = ctypes.CDLL(None, use_errno=True)
libc.fallocate.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_long, ctypes.c_long]
PUNCH_HOLE_KEEP_SIZE = 0x02 | 0x01
for b in range(1, BLOCKS, 2):
    if libc.fallocate(fd, PUNCH_HOLE_KEEP_SIZE, b * BS, BS) != 0:
        raise OSError(ctypes.get_errno(), "fallocate punch-hole")
os.fsync(fd)
os.close(fd)
PY

sync
umount "$MNT"
trap - EXIT
rmdir "$MNT"

# The image is only worth having if the tree really is two levels deep.
# xfs_db reads the AG headers the kernel wrote; if no group's rmap tree
# reached level 2 this fixture would test nothing, so it refuses.
agcount=$(xfs_db -r -c 'sb 0' -c 'print agcount' "$DEEP" | sed -n 's/^agcount = //p')
deepest=0
for ((ag = 0; ag < agcount; ag++)); do
  level=$(xfs_db -r -c "agf $ag" -c 'print rmaplevel' "$DEEP" | sed -n 's/^rmaplevel = //p')
  [ "${level:-0}" -gt "$deepest" ] && deepest=$level
done
echo "xfs-rmap-deep.img: deepest rmap tree is $deepest level(s) over $agcount group(s)"
[ "$deepest" -ge 2 ] || {
  echo "xfs-rmap-deep.img: no group's rmap tree is more than one block deep; the fixture would not test what it is for" >&2
  exit 1
}

echo "built $DEEP"
ls -la "$DEEP"
