//! xfs-win-driver CLI entry point.

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

use fs_core::{BlockRead, FileDevice};
use fs_xfs::Filesystem;
use winfsp_fs_skeleton::FsBackend;

// We use the library re-exports so integration tests under `tests/`
// can reach the same modules without a duplicate `#[path]` shim.
use xfs_win_driver::{mount, probe};

struct XfsBackend;

impl FsBackend for XfsBackend {
    const FS_NAME: &'static str = "xfs";
    const SERVICE_NAME: &'static str = "XfsWatcher";
    const LAUNCHER_SERVICE_CLASS: &'static str = "xfs-mount";
    const FILE_EXTENSION: &'static str = "img";

    fn detect(bytes: &[u8]) -> bool {
        probe::is_xfs(bytes)
    }
}

#[derive(Parser)]
#[command(
    name = "xfs",
    about = "Browse and (eventually) mount XFS volumes on Windows"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct ImgArg {
    /// XFS filesystem image (file path).
    image: PathBuf,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print volume info (block size, root NID, volume name).
    Info(ImgArg),
    /// List directory entries at PATH (default `/`).
    Ls {
        #[command(flatten)]
        img: ImgArg,
        #[arg(default_value = "/")]
        path: String,
    },
    /// Print a regular file's contents to stdout.
    Cat {
        #[command(flatten)]
        img: ImgArg,
        path: String,
    },
    /// Foreground auto-mount watcher (development).
    Watch,
    /// SCM service variant -- started by `sc start XfsWatcher`.
    #[cfg(windows)]
    Service,
    /// Mount one partition. Spawned by Watch/Service per detected hit.
    /// On Windows with `--features mount`, dispatches into the WinFsp
    /// adapter; on non-Windows or when the feature is off, prints a
    /// hint and exits cleanly so the same binary builds everywhere.
    Mount {
        disk: String,
        #[arg(long)]
        drive: String,
        #[arg(long)]
        part: Option<usize>,
        /// Force the mount read-only: rejects every write callback
        /// with `STATUS_MEDIA_WRITE_PROTECTED` and sets WinFsp's
        /// `read_only_volume` flag so the cache manager short-circuits
        /// writes at the kernel level. Without this flag the mount is
        /// writable: writes are staged in an in-memory overlay and the
        /// underlay XFS image is never touched.
        #[arg(long)]
        ro: bool,
        /// Discard the in-memory overlay on dismount (default). The
        /// canonical "read-only volume that pretended to be writable"
        /// behaviour — every staged write disappears at unmount.
        #[arg(long, conflicts_with = "scratch_sidecar")]
        scratch_discard: bool,
        /// Serialise the overlay state to a JSON sidecar at the given
        /// path on dismount. Useful for replay / audit. Requires the
        /// `overlay-sidecar` Cargo feature; without it the binary
        /// emits a clear error at unmount time rather than silently
        /// dropping the data.
        #[arg(long)]
        scratch_sidecar: Option<PathBuf>,
        // NO --scratch-rebuild. The sibling erofs driver has one: it
        // serialises the overlay into a fresh image via that reader's
        // mkfs. am-fs-xfs has no mkfs module, so there is nothing to
        // build an image with, and a flag that always fails is worse
        // than a flag that is absent -- it appears in --help as though
        // it might work.
    },
}

fn open_fs(img: &PathBuf) -> Result<Filesystem> {
    let dev = FileDevice::open(img).with_context(|| format!("opening {}", img.display()))?;
    let dev: Arc<dyn BlockRead> = Arc::new(dev);
    Filesystem::mount(dev).map_err(|e| anyhow!("mount XFS: {e}"))
}

fn cmd_info(img: &PathBuf) -> Result<()> {
    let fs = open_fs(img)?;
    let sb = fs.superblock();
    // These are XFS's fields, not EROFS's. The port arrived here with
    // `root_nid`, `meta_blkaddr` and `xattr_blkaddr` printed -- EROFS
    // concepts with no XFS counterpart, produced by renaming
    // identifiers in a file whose content was never about XFS. Same
    // class of error the probe had, and the same reason it compiled:
    // nothing is syntactically wrong with printing a field that means
    // something else.
    println!("version          v{}", sb.version());
    println!("block size       {}", sb.blocksize);
    println!("sector size      {}", sb.sectsize);
    println!("inode size       {}", sb.inodesize);
    println!("data blocks      {}", sb.dblocks);
    println!("free blocks      {}", sb.fdblocks);
    println!("inodes           {} ({} free)", sb.icount, sb.ifree);
    println!("root inode       {}", sb.rootino);
    println!("AGs              {} x {} blocks", sb.agcount, sb.agblocks);
    println!(
        "log              {}",
        if sb.has_internal_log() {
            format!("internal, {} blocks at {}", sb.logblocks, sb.logstart)
        } else {
            "external".to_string()
        }
    );
    println!("volume name      {:?}", sb.fname);
    println!("features2        0x{:08X}", sb.features2);
    println!("feature_compat   0x{:08X}", sb.features_compat);
    println!("feature_ro_compat 0x{:08X}", sb.features_ro_compat);
    Ok(())
}

fn cmd_ls(img: &PathBuf, path: &str) -> Result<()> {
    let fs = open_fs(img)?;
    let dir = fs
        .lookup_path(path)
        .map_err(|e| anyhow!("lookup {path}: {e}"))?;
    // XFS threads the raw inode fork through read_dir: a directory's
    // entries can live inline in the inode itself (short form), so the
    // parsed Inode alone is not enough.
    let (dir, raw) = fs
        .read_inode_raw(dir.ino)
        .map_err(|e| anyhow!("read inode {path}: {e}"))?;
    let entries = fs
        .read_dir(&dir, &raw)
        .map_err(|e| anyhow!("read_dir {path}: {e}"))?;
    for e in entries {
        // Names are bytes: XFS neither NUL-terminates them nor requires
        // valid UTF-8.
        let name = String::from_utf8_lossy(&e.name);
        // `ftype` is None when the filesystem does not record one, which
        // is a fact about the image rather than an error.
        let kind = match e.ftype {
            Some(t) => format!("{t:?}"),
            None => "?".to_string(),
        };
        println!("{:<10} {:>10} {}", kind, e.ino, name);
    }
    Ok(())
}

fn cmd_cat(img: &PathBuf, path: &str) -> Result<()> {
    use std::io::Write;
    let fs = open_fs(img)?;
    let inode = fs
        .lookup_path(path)
        .map_err(|e| anyhow!("lookup {path}: {e}"))?;
    if !inode.is_regular_file() {
        return Err(anyhow!("{path} is not a regular file"));
    }
    let (inode, raw) = fs
        .read_inode_raw(inode.ino)
        .map_err(|e| anyhow!("read inode {path}: {e}"))?;
    let buf = fs
        .read_file(&inode, &raw)
        .map_err(|e| anyhow!("read {path}: {e}"))?;
    std::io::stdout().write_all(&buf)?;
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Info(a) => cmd_info(&a.image),
        Cmd::Ls { img, path } => cmd_ls(&img.image, &path),
        Cmd::Cat { img, path } => cmd_cat(&img.image, &path),
        Cmd::Watch => winfsp_fs_skeleton::watch::run::<XfsBackend>(),
        #[cfg(windows)]
        Cmd::Service => winfsp_fs_skeleton::service::run::<XfsBackend>(),
        Cmd::Mount {
            disk,
            drive,
            part,
            ro,
            scratch_discard,
            scratch_sidecar,
        } => cmd_mount(&disk, &drive, part, ro, scratch_discard, scratch_sidecar),
    }
}

/// Open the given disk/partition as an XFS filesystem and mount it on
/// `drive` via WinFsp. Blocks until Ctrl-C. On non-Windows hosts (or
/// when built without `--features mount`), `Mount::run` prints a
/// diagnostic and returns Ok so cross-platform `cargo build` succeeds.
fn cmd_mount(
    disk: &str,
    drive: &str,
    part: Option<usize>,
    ro: bool,
    _scratch_discard: bool,
    scratch_sidecar: Option<PathBuf>,
) -> Result<()> {
    let path = PathBuf::from(disk);
    let policy = if let Some(p) = scratch_sidecar {
        mount::DismountPolicy::Sidecar(p)
    } else {
        // Either `--scratch-discard` was passed explicitly, or no
        // `--scratch-*` flag was supplied. Either way the policy is
        // Discard. The flag still has documentation value (and clap
        // surfaces it in `--help`).
        mount::DismountPolicy::Discard
    };
    let write_mode = if ro {
        mount::WriteMode::ReadOnly
    } else {
        mount::WriteMode::Writable
    };
    let m = mount::Mount::open(&path, part)
        .with_context(|| format!("opening XFS on {disk}"))?
        .with_dismount_policy(policy)
        .with_write_mode(write_mode);
    m.run(drive)
}
