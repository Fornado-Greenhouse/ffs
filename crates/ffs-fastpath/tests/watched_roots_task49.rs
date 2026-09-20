//! task_49 / ADR-036: the watcher's notion of "mine". The daemon watches
//! the data dir root and asks `is_watched_root` per event, so the set of
//! roots is the registry's projection families plus `inbox/`, never a
//! constant in code. Loads the starter library so the assertion tracks
//! the shipped `[path]` tables.

use std::path::PathBuf;

use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::path::FamilyTable;
use ffs_fastpath::is_watched_root;

fn starter_table() -> FamilyTable {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    let registry = SpecRegistry::new();
    registry
        .load_dir(&root.join("starter").join("predicates"))
        .unwrap();
    FamilyTable::from_registry(&registry)
}

#[test]
fn every_starter_family_and_inbox_are_watched() {
    let table = starter_table();
    for rel in [
        "contacts/by-name/S/Sara_Chen.md",
        "people/by-name/P/Pat_Example.md",
        "notes/by-date/2026/Plain_note.md",
        "orgs/by-name/A/Acme_Widgets.md",
        "articles/by-date/2026/Widget_maker.md",
        "events/by-date/2026/Groundbreaking.md",
        "briefings/2026-09-20.md",
        "inbox/2026-09-20.md",
    ] {
        assert!(is_watched_root(rel, &table), "{rel} is watched");
    }
}

#[test]
fn everything_else_under_the_data_dir_is_not() {
    let table = starter_table();
    for rel in [
        "ingest/dropped.md",
        "ingest/.processed/dropped.md",
        "run/ffs.sock",
        "log/daemon.log",
        "skills/scribe/SKILL.md",
        "config/predicates/contact.person.toml",
        ".obsidian/workspace.json",
        ".courier/ledger.json",
        "contacts/.Sara_Chen.md.tmp",
        "inbox/.2026-09-20.md.tmp",
        "",
    ] {
        assert!(!is_watched_root(rel, &table), "{rel} is not watched");
    }
}
