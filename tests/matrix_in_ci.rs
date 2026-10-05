//! The Windows matrix runs in CI, on the current test harness, against the
//! images this repository builds (#7).
//!
//! Until now `test-matrix.json` ran only on a developer's machine with a
//! Windows VM, through the old vendored `fs-test-harness` (v3), against
//! images from a `../rust-fs-xfs/test-disks` script that no longer exists.
//! So nothing in CI had ever mounted an image with the driver. These
//! checks read the files as text; they need no Windows.

use std::fs;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn exists(rel: &str) -> bool {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel).exists()
}

#[test]
fn the_harness_is_fs_windows_test_harness_at_a_v4_release() {
    let chores = read("chores.yml");
    assert!(
        chores.contains(
            "HARNESS_URL: https://github.com/antimatter-studios/fs-windows-test-harness.git"
        ),
        "chores.yml does not pin fs-windows-test-harness"
    );
    let reference = chores
        .lines()
        .find_map(|l| l.trim().strip_prefix("HARNESS_REF:"))
        .expect("chores.yml has a HARNESS_REF")
        .trim()
        .to_string();
    assert!(
        reference.starts_with("v4."),
        "HARNESS_REF is {reference}, not a v4 release"
    );
    assert!(
        exists("fs-windows-test-harness.toml"),
        "no fs-windows-test-harness.toml"
    );
    assert!(
        read("fs-windows-test-harness.toml").contains(r#"binary      = "target/release/xfs.exe""#),
        "fs-windows-test-harness.toml does not name target/release/xfs.exe"
    );
    assert!(
        !exists("harness.toml"),
        "the v3 harness.toml is still here beside the v4 config"
    );
}

#[test]
fn every_scenario_runs_on_an_image_this_repository_builds() {
    let matrix = read("test-matrix.json");
    assert!(
        !matrix.contains("rust-fs-xfs/test-disks"),
        "test-matrix.json still points at ../rust-fs-xfs/test-disks, which no longer exists"
    );
    // Each scenario's first step builds its image; the op is defined in the
    // harness config.
    let scenarios = matrix.matches("\"recipe\"").count();
    let builds = matrix.matches("\"op\": \"build-xfs-image\"").count();
    assert!(
        scenarios > 0,
        "test-matrix.json has no scenarios with a recipe"
    );
    assert_eq!(
        builds, scenarios,
        "a scenario does not start by building its image"
    );
    assert!(
        read("fs-windows-test-harness.toml").contains("[ops.build-xfs-image]"),
        "the harness config defines no build-xfs-image op"
    );
}

#[test]
fn ci_runs_the_matrix_on_windows_with_a_floor() {
    // The matrix's jobs live in matrix.yml, which ci.yml calls (#7).
    let ci = read(".github/workflows/matrix.yml");
    assert!(
        ci.contains("scripts/run-matrix.sh"),
        "matrix.yml does not run the matrix"
    );
    assert!(
        ci.contains("scripts/matrix-floor.sh"),
        "matrix.yml does not hold the matrix to a floor of executed scenarios"
    );
    assert!(exists("scripts/run-matrix.sh"), "no scripts/run-matrix.sh");
    assert!(
        exists("scripts/matrix-floor.sh"),
        "no scripts/matrix-floor.sh"
    );
}
