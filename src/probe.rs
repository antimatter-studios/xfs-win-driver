//! XFS-specific superblock detection.
//!
//! XFS puts its superblock at byte offset **0** — the very start of the
//! device — and its magic is the ASCII `XFSB` stored **big-endian**
//! (`0x5846_5342`). Both facts differ from every other filesystem in
//! this family, which is worth stating because this file began as a
//! copy of the EROFS driver's probe and every one of those details was
//! wrong in the copy: EROFS's superblock is at offset 1024 with a
//! little-endian `0xE0F5_E1E2`. A rename pass produced code that read
//! plausibly, cited a kernel header that does not define the constant
//! it named, and would never have matched an XFS volume.
//!
//! The value here is taken from `fs_xfs::superblock::XFS_SB_MAGIC`
//! rather than retyped, so the driver and the library it drives cannot
//! disagree about what an XFS filesystem looks like.

/// Byte offset of the XFS superblock. Zero, unlike EROFS (1024) or
/// ext4 (1024).
const XFS_SUPER_OFFSET: usize = 0;

/// `XFSB`, big-endian on disk. Re-exported from the reader crate so
/// there is one definition.
const XFS_MAGIC: [u8; 4] = fs_xfs::superblock::XFS_SB_MAGIC.to_be_bytes();

pub fn is_xfs(bytes: &[u8]) -> bool {
    if bytes.len() < XFS_SUPER_OFFSET + 4 {
        return false;
    }
    bytes[XFS_SUPER_OFFSET..XFS_SUPER_OFFSET + 4] == XFS_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BUF_LEN: usize = 1100;

    #[test]
    fn matches_magic_at_super_offset() {
        let mut buf = vec![0u8; TEST_BUF_LEN];
        buf[XFS_SUPER_OFFSET..XFS_SUPER_OFFSET + 4].copy_from_slice(&XFS_MAGIC);
        assert!(is_xfs(&buf));
    }

    /// The magic is `XFSB` as ASCII, which is what a hex dump of a real
    /// volume shows. Written out literally so a wrong constant in the
    /// library would fail here rather than being inherited silently.
    #[test]
    fn the_magic_is_the_ascii_xfsb() {
        assert_eq!(&XFS_MAGIC, b"XFSB");
    }

    /// The superblock is at offset 0, not 1024. This is the assertion
    /// the copied-from-EROFS version would have failed.
    #[test]
    fn the_superblock_is_at_the_start_of_the_device() {
        assert_eq!(XFS_SUPER_OFFSET, 0);
        let mut buf = vec![0u8; TEST_BUF_LEN];
        buf[1024..1028].copy_from_slice(&XFS_MAGIC);
        assert!(
            !is_xfs(&buf),
            "magic at 1024 is EROFS's layout, not XFS's — this must not match"
        );
    }

    #[test]
    fn rejects_a_buffer_that_is_too_short() {
        assert!(!is_xfs(&[]));
        assert!(!is_xfs(&XFS_MAGIC[..3]));
    }

    #[test]
    fn rejects_a_zeroed_buffer() {
        assert!(!is_xfs(&vec![0u8; TEST_BUF_LEN]));
    }
}
