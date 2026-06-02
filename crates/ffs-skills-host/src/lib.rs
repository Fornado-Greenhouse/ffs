//! `ffs-skills-host` — subprocess host for Python skills (scribe,
//! librarian, auditor). The daemon embeds this crate to discover
//! skills under `~/.ffs/skills/`, spawn each as a Python subprocess,
//! supervise crashes with exponential backoff, enforce per-call
//! timeouts, and broker substrate-access queries from skills to the
//! daemon's JSON-RPC layer with the skill's identity.
//!
//! See ADR-009 (SKILL.md claw-pattern contract) and ADR-015
//! (in-process minimal host scope). Per-skill protocol is documented
//! in `protocol.rs`; the Python author-side helper library lives at
//! `skills/_lib/ffs_skill.py` and handles the framing for skills.

use std::path::Path;
use std::sync::Arc;

pub mod protocol;
pub mod registry;
pub mod subprocess;

pub use protocol::{HostToSkill, ProtocolError, SkillToHost};
pub use registry::{RegistryError, SkillKind, SkillManifest, SkillRegistry};
pub use subprocess::{
    BACKOFF_CAP, BACKOFF_INITIAL, RefuseAllProxy, SHUTDOWN_GRACE, SkillError, SkillProcess,
    SubstrateAccess,
};

/// Crate identity used by smoke tests and load-order verification.
pub const CRATE_NAME: &str = "ffs-skills-host";

/// Owning collection: discovers skills from a drop-in directory and
/// holds one running `SkillProcess` per registered skill. The daemon
/// constructs one `SkillsHost` at startup.
pub struct SkillsHost {
    proxy: Arc<dyn SubstrateAccess>,
    skills: Vec<SkillProcess>,
}

impl SkillsHost {
    pub fn new(proxy: Arc<dyn SubstrateAccess>) -> Self {
        Self {
            proxy,
            skills: Vec::new(),
        }
    }

    /// Discover skills under `dir` and spawn each. Skills with a
    /// malformed `SKILL.md` are skipped with a warning so one bad
    /// skill cannot prevent the daemon from starting.
    pub fn discover_and_spawn(&mut self, dir: &Path) -> Result<(), RegistryError> {
        let mut registry = SkillRegistry::new();
        registry.discover(dir)?;
        self.spawn_from_registry(&registry);
        Ok(())
    }

    /// Spawn supervisors for every manifest in `registry`. Lets the
    /// daemon binary discover separately (so it can apply env-var
    /// overrides like `FFS_SKILL_TIMEOUT_MS`) and spawn afterward.
    pub fn spawn_from_registry(&mut self, registry: &SkillRegistry) {
        for manifest in registry.skills() {
            let proc = SkillProcess::spawn(manifest.clone(), self.proxy.clone());
            self.skills.push(proc);
        }
    }

    pub fn skills(&self) -> &[SkillProcess] {
        &self.skills
    }

    pub fn get(&self, name: &str) -> Option<&SkillProcess> {
        self.skills.iter().find(|s| s.manifest.name == name)
    }

    /// Polite shutdown: signal every skill's supervisor. The
    /// supervisors send a `Shutdown` frame, wait up to
    /// `SHUTDOWN_GRACE`, then SIGKILL. Returns after all supervisors
    /// have been signalled — does NOT block on actual exit (the
    /// supervisors are async).
    pub fn shutdown_all(&self) {
        for s in &self.skills {
            s.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(CRATE_NAME, "ffs-skills-host");
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
