//! Installer-layout regression test (task_22).
//!
//! Runs `installer/install.sh --skip-service --skip-plugin` against
//! a freshly-built release binary tree and asserts the produced
//! `~/.ffs/` layout matches the contract the daemon expects.
//!
//! This is the closest faithful surrogate to "install on a clean
//! VM" the workspace can run inside nextest. It catches:
//!
//! - The installer's source-discovery logic (predicates dir, skills
//!   dir, binaries).
//! - The starter library's filename layout.
//! - Idempotence: re-running the installer doesn't blow up
//!   user-edited files.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

/// Only run when release binaries exist — building them on demand
/// inside a test takes 30+ seconds and is best left to the
/// workspace-level CI pipeline. Without them we just skip.
fn release_bins_exist() -> bool {
    let r = repo_root();
    r.join("target/release/ffs").exists()
        && r.join("target/release/ffs-daemon").exists()
        && r.join("target/release/ffs-mcp").exists()
}

#[test]
fn installer_seeds_data_dir_and_places_binaries() {
    if !release_bins_exist() {
        eprintln!(
            "skipping installer_seeds_data_dir_and_places_binaries — release bins missing; \
             run `cargo build --release -p ffs -p ffs-daemon -p ffs-mcp` first"
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let prefix = tmp.path().join("prefix");
    std::fs::create_dir_all(&home).unwrap();

    let installer = repo_root().join("installer").join("install.sh");
    let status = Command::new("bash")
        .arg(&installer)
        .arg("--skip-service")
        .arg("--skip-plugin")
        .env("HOME", &home)
        .env("FFS_PREFIX", &prefix)
        .env_remove("FFS_VAULT")
        .status()
        .expect("run installer");
    assert!(status.success(), "installer exited {status:?}");

    // Binaries landed where expected.
    for name in ["ffs", "ffs-daemon", "ffs-mcp"] {
        let p = prefix.join("bin").join(name);
        assert!(p.exists(), "missing bin: {}", p.display());
    }

    // Starter library seeded.
    let data_dir = home.join(".ffs");
    assert!(
        data_dir
            .join("config/predicates/contact.person.toml")
            .exists()
    );
    assert!(data_dir.join("config/predicates/note.toml").exists());
    assert!(
        data_dir
            .join("config/predicates/person.generic.toml")
            .exists()
    );
    assert!(
        data_dir
            .join("config/templates/contact-person.md.tera")
            .exists()
    );

    // Skills bundles landed.
    assert!(data_dir.join("skills/auditor").is_dir());
    assert!(data_dir.join("skills/librarian").is_dir());
    assert!(data_dir.join("skills/scribe").is_dir());

    // Re-run is idempotent: preserves user-edited content.
    let pred_path = data_dir.join("config/predicates/contact.person.toml");
    std::fs::write(&pred_path, b"# user-edited marker\n").unwrap();
    let status2 = Command::new("bash")
        .arg(&installer)
        .arg("--skip-service")
        .arg("--skip-plugin")
        .env("HOME", &home)
        .env("FFS_PREFIX", &prefix)
        .env_remove("FFS_VAULT")
        .status()
        .expect("re-run installer");
    assert!(status2.success(), "re-run exited {status2:?}");
    let after = std::fs::read_to_string(&pred_path).unwrap();
    assert_eq!(
        after, "# user-edited marker\n",
        "installer overwrote a user-edited predicate spec"
    );
}
