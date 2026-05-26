//! Skill discovery. Walks `~/.ffs/skills/<name>/` and parses each
//! directory's `SKILL.md` into a `SkillManifest`. The `SKILL.md`
//! frontmatter shape conforms to ADR-009's claw-pattern contract so
//! the same skill bundles can run inside OpenClaw or Hermes later.
//!
//! Minimal MVP frontmatter (YAML between leading `---` lines):
//!
//! ```text
//! ---
//! name: scribe
//! kind: scribe
//! entry_point: extraction.py
//! python: python3
//! timeout_ms: 30000
//! ---
//! ```
//!
//! `kind` is one of `scribe`, `librarian`, `auditor` for MVP; any
//! other value parses to `SkillKind::Other(_)`. The host treats all
//! kinds uniformly — `kind` is just a label for logs and the auditor.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("missing SKILL.md in {0}")]
    MissingManifest(PathBuf),
    #[error("malformed SKILL.md frontmatter in {path}: {reason}")]
    BadFrontmatter { path: PathBuf, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillKind {
    Scribe,
    Librarian,
    Auditor,
    Other(String),
}

impl SkillKind {
    fn from_str(s: &str) -> Self {
        match s {
            "scribe" => Self::Scribe,
            "librarian" => Self::Librarian,
            "auditor" => Self::Auditor,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Scribe => "scribe",
            Self::Librarian => "librarian",
            Self::Auditor => "auditor",
            Self::Other(s) => s.as_str(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillManifest {
    pub name: String,
    pub kind: SkillKind,
    /// Path to the Python script (relative to the skill dir).
    pub entry_point: PathBuf,
    /// Python interpreter to invoke. Defaults to `python3`.
    pub python: String,
    /// Per-call timeout. The host kills + restarts the skill if a
    /// single invocation exceeds this. Defaults to 30 seconds.
    pub timeout: Duration,
    /// Absolute path to the skill's directory; the entry point is
    /// resolved against this and the process's cwd is set to this.
    pub dir: PathBuf,
}

impl SkillManifest {
    pub fn entry_point_abs(&self) -> PathBuf {
        self.dir.join(&self.entry_point)
    }
}

#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: Vec<SkillManifest>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn skills(&self) -> &[SkillManifest] {
        &self.skills
    }

    pub fn get(&self, name: &str) -> Option<&SkillManifest> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Walk one level of children under `dir`. Each child directory
    /// containing a `SKILL.md` is parsed into a `SkillManifest`.
    /// Children without `SKILL.md` are skipped silently — the host
    /// treats `~/.ffs/skills/` as a drop-in folder.
    pub fn discover(&mut self, dir: &Path) -> Result<(), RegistryError> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("SKILL.md");
            if !manifest_path.exists() {
                continue;
            }
            let raw = std::fs::read_to_string(&manifest_path)?;
            let manifest = parse_manifest(&raw, &path)?;
            self.skills.push(manifest);
        }
        Ok(())
    }
}

fn parse_manifest(raw: &str, dir: &Path) -> Result<SkillManifest, RegistryError> {
    let trimmed = raw.trim_start();
    let body = trimmed
        .strip_prefix("---")
        .ok_or_else(|| RegistryError::BadFrontmatter {
            path: dir.join("SKILL.md"),
            reason: "missing leading `---`".into(),
        })?;
    // Find the closing `---` on its own line.
    let mut iter = body.lines();
    let mut fm_lines = Vec::new();
    let mut closed = false;
    // Skip leading newline after `---`.
    for line in &mut iter {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        fm_lines.push(line);
    }
    if !closed {
        return Err(RegistryError::BadFrontmatter {
            path: dir.join("SKILL.md"),
            reason: "missing closing `---`".into(),
        });
    }

    let mut name: Option<String> = None;
    let mut kind: Option<SkillKind> = None;
    let mut entry_point: Option<PathBuf> = None;
    let mut python = "python3".to_string();
    let mut timeout = Duration::from_secs(30);

    for line in fm_lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| RegistryError::BadFrontmatter {
                path: dir.join("SKILL.md"),
                reason: format!("missing `:` in frontmatter line: {line}"),
            })?;
        let k = k.trim();
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k {
            "name" => name = Some(v.to_string()),
            "kind" => kind = Some(SkillKind::from_str(v)),
            "entry_point" => entry_point = Some(PathBuf::from(v)),
            "python" => python = v.to_string(),
            "timeout_ms" => {
                let ms = v
                    .parse::<u64>()
                    .map_err(|_| RegistryError::BadFrontmatter {
                        path: dir.join("SKILL.md"),
                        reason: format!("timeout_ms must be an integer, got {v}"),
                    })?;
                timeout = Duration::from_millis(ms);
            }
            _ => {} // Unknown keys are tolerated for forward compatibility.
        }
    }

    let name = name.ok_or_else(|| RegistryError::BadFrontmatter {
        path: dir.join("SKILL.md"),
        reason: "missing `name`".into(),
    })?;
    let entry_point = entry_point.ok_or_else(|| RegistryError::BadFrontmatter {
        path: dir.join("SKILL.md"),
        reason: "missing `entry_point`".into(),
    })?;
    let kind = kind.unwrap_or_else(|| SkillKind::Other("unknown".into()));

    Ok(SkillManifest {
        name,
        kind,
        entry_point,
        python,
        timeout,
        dir: dir.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let raw = "---\nname: scribe\nkind: scribe\nentry_point: extraction.py\n---\n";
        let m = parse_manifest(raw, Path::new("/tmp/skill")).unwrap();
        assert_eq!(m.name, "scribe");
        assert_eq!(m.kind, SkillKind::Scribe);
        assert_eq!(m.entry_point, PathBuf::from("extraction.py"));
        assert_eq!(m.python, "python3");
        assert_eq!(m.timeout, Duration::from_secs(30));
    }

    #[test]
    fn parses_full_manifest_with_overrides() {
        let raw = "---\nname: \"my-skill\"\nkind: librarian\nentry_point: 'bin/run.py'\npython: python3.12\ntimeout_ms: 5000\n# a comment\n---\nbody text ignored\n";
        let m = parse_manifest(raw, Path::new("/tmp/skill")).unwrap();
        assert_eq!(m.name, "my-skill");
        assert_eq!(m.kind, SkillKind::Librarian);
        assert_eq!(m.entry_point, PathBuf::from("bin/run.py"));
        assert_eq!(m.python, "python3.12");
        assert_eq!(m.timeout, Duration::from_millis(5000));
    }

    #[test]
    fn unknown_kind_becomes_other() {
        let raw = "---\nname: x\nkind: courier\nentry_point: x.py\n---\n";
        let m = parse_manifest(raw, Path::new("/tmp/skill")).unwrap();
        assert_eq!(m.kind, SkillKind::Other("courier".into()));
    }

    #[test]
    fn missing_required_field_errors() {
        let raw = "---\nkind: scribe\nentry_point: x.py\n---\n";
        let err = parse_manifest(raw, Path::new("/tmp/skill")).unwrap_err();
        assert!(matches!(err, RegistryError::BadFrontmatter { .. }));
    }

    #[test]
    fn missing_frontmatter_errors() {
        let raw = "no frontmatter here\n";
        let err = parse_manifest(raw, Path::new("/tmp/skill")).unwrap_err();
        assert!(matches!(err, RegistryError::BadFrontmatter { .. }));
    }

    #[test]
    fn discover_walks_drop_in_directory() {
        let tmp = tempfile::tempdir().unwrap();
        // Skill A: well-formed
        let a = tmp.path().join("scribe");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(
            a.join("SKILL.md"),
            "---\nname: scribe\nkind: scribe\nentry_point: x.py\n---\n",
        )
        .unwrap();
        // Skill B: directory without SKILL.md is silently skipped
        std::fs::create_dir_all(tmp.path().join("orphan")).unwrap();
        // Skill C: well-formed
        let c = tmp.path().join("librarian");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(
            c.join("SKILL.md"),
            "---\nname: librarian\nkind: librarian\nentry_point: x.py\n---\n",
        )
        .unwrap();

        let mut reg = SkillRegistry::new();
        reg.discover(tmp.path()).unwrap();
        assert_eq!(reg.skills().len(), 2);
        assert!(reg.get("scribe").is_some());
        assert!(reg.get("librarian").is_some());
        assert!(reg.get("orphan").is_none());
    }

    #[test]
    fn discover_missing_dir_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = SkillRegistry::new();
        reg.discover(&tmp.path().join("does-not-exist")).unwrap();
        assert!(reg.skills().is_empty());
    }
}
