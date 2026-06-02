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

/// task_30: when invoked without `--skip-plugin` and without a
/// `--vault` argument, the installer defaults to substrate-is-vault
/// and seeds `$DATA_DIR/.obsidian/plugins/ffs/` (rather than asking
/// the user to pre-open the vault in Obsidian). Plugin source has
/// to exist at the workspace `obsidian-plugin/` for this test —
/// developer machines have it; we skip cleanly when it's missing.
#[test]
fn installer_default_seeds_substrate_is_vault_obsidian_layout() {
    if !release_bins_exist() {
        eprintln!("skipping — release bins missing");
        return;
    }
    let plugin_src = repo_root().join("obsidian-plugin").join("main.js");
    if !plugin_src.exists() {
        eprintln!(
            "skipping — obsidian-plugin/main.js missing; run `npm run build` in obsidian-plugin/"
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
        .env("HOME", &home)
        .env("FFS_PREFIX", &prefix)
        .env_remove("FFS_VAULT")
        .status()
        .expect("run installer");
    assert!(status.success(), "installer exited {status:?}");

    let data_dir = home.join(".ffs");
    let plugin_dir = data_dir.join(".obsidian/plugins/ffs");
    assert!(
        plugin_dir.is_dir(),
        "substrate-is-vault plugin dir missing: {}",
        plugin_dir.display()
    );
    assert!(
        plugin_dir.join("main.js").exists(),
        "plugin main.js missing under {}",
        plugin_dir.display()
    );
    assert!(
        plugin_dir.join("manifest.json").exists(),
        "plugin manifest.json missing under {}",
        plugin_dir.display()
    );
}

/// task_30: `--vault $DATA_DIR` explicitly produces the same layout
/// as no `--vault` arg — no double-nesting like
/// `~/.ffs/.obsidian/.obsidian/plugins/ffs/`.
#[test]
fn installer_with_explicit_vault_equal_to_data_dir_is_idempotent_with_default() {
    if !release_bins_exist() {
        eprintln!("skipping — release bins missing");
        return;
    }
    if !repo_root().join("obsidian-plugin").join("main.js").exists() {
        eprintln!("skipping — obsidian-plugin/main.js missing");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let prefix = tmp.path().join("prefix");
    std::fs::create_dir_all(&home).unwrap();
    let data_dir = home.join(".ffs");

    let installer = repo_root().join("installer").join("install.sh");
    let status = Command::new("bash")
        .arg(&installer)
        .arg("--skip-service")
        .arg("--vault")
        .arg(&data_dir)
        .env("HOME", &home)
        .env("FFS_PREFIX", &prefix)
        .env_remove("FFS_VAULT")
        .status()
        .expect("run installer");
    assert!(status.success(), "installer exited {status:?}");

    let plugin_dir = data_dir.join(".obsidian/plugins/ffs");
    assert!(
        plugin_dir.is_dir(),
        "plugin dir missing: {}",
        plugin_dir.display()
    );
    // Negative: no double-nested path
    assert!(
        !data_dir.join(".obsidian/.obsidian").exists(),
        "double-nested .obsidian directory should not exist"
    );
}

/// task_30: when `--vault` points somewhere other than the data
/// dir, the installer still writes the plugin there for backwards
/// compatibility, but warns to stderr/stdout about the
/// non-canonical location.
#[test]
fn installer_with_external_vault_warns_about_non_canonical_location() {
    if !release_bins_exist() {
        eprintln!("skipping — release bins missing");
        return;
    }
    if !repo_root().join("obsidian-plugin").join("main.js").exists() {
        eprintln!("skipping — obsidian-plugin/main.js missing");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let prefix = tmp.path().join("prefix");
    let external_vault = tmp.path().join("external-vault");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&external_vault).unwrap();

    let installer = repo_root().join("installer").join("install.sh");
    let output = Command::new("bash")
        .arg(&installer)
        .arg("--skip-service")
        .arg("--vault")
        .arg(&external_vault)
        .env("HOME", &home)
        .env("FFS_PREFIX", &prefix)
        .env_remove("FFS_VAULT")
        .output()
        .expect("run installer");
    assert!(
        output.status.success(),
        "installer should succeed (just warn): exit {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    // Plugin landed at the external location (backwards compat).
    assert!(
        external_vault
            .join(".obsidian/plugins/ffs/main.js")
            .exists(),
        "plugin should still install at the external vault for backwards compat"
    );
    // And a warning fired (the installer writes WARN lines to
    // stdout via `say`, not stderr, so check both streams).
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("WARN") && combined.contains("substrate root"),
        "expected substrate-root warning in installer output; got:\n{combined}"
    );
}
