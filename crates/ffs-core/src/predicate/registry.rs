//! In-memory registry of loaded predicate specs with optional filesystem
//! hot-reload.
//!
//! `SpecRegistry` is the runtime view of `~/.ffs/config/predicates/`: a
//! map from predicate name to parsed spec, plus per-predicate compiled
//! JSON Schema validators with parent-predicate inheritance composed via
//! JSON Schema `allOf`.
//!
//! Concurrent access pattern: a single `RwLock` serializes writes (load,
//! unload, reload) and lets reads (`get`, `validate_claim`) proceed
//! concurrently. The watcher thread takes the write lock when filesystem
//! events arrive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use thiserror::Error;

use super::PredicateSpec;
use super::mod_helpers::{
    compile_validator, effective_schema, parse_spec_str, validate_reverse_map,
};

#[derive(Debug, Error)]
pub enum SpecError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("toml parse error in {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid JSON Schema in predicate '{predicate}': {message}")]
    InvalidJsonSchema { predicate: String, message: String },
    #[error("reverse-map rule for predicate '{predicate}' references undefined output '{output}'")]
    UndefinedReverseMapOutput { predicate: String, output: String },
    #[error("predicate '{predicate}' inherits from '{parent}' which is not loaded")]
    UnknownParent { predicate: String, parent: String },
    #[error("watcher error: {0}")]
    Watcher(String),
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("predicate '{0}' is not registered")]
    UnknownPredicate(String),
    #[error("claim does not validate against predicate schema: {0}")]
    SchemaValidation(String),
}

#[derive(Default)]
struct RegistryInner {
    specs: HashMap<String, PredicateSpec>,
    /// Map from absolute file path to predicate name. Lets us undo on file removal.
    paths: HashMap<PathBuf, String>,
    /// Compiled, parent-inherited JSON Schema validators per predicate.
    validators: HashMap<String, Arc<jsonschema::Validator>>,
}

#[derive(Clone, Default)]
pub struct SpecRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

/// Drop guard for an active filesystem watcher.
pub struct WatchHandle {
    _watcher: notify::RecommendedWatcher,
}

impl SpecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load every `*.toml` file in `dir` into the registry. Two-pass:
    /// first parse all files, then resolve parent links and compile
    /// validators in parent-first order.
    pub fn load_dir(&self, dir: &Path) -> Result<(), SpecError> {
        let entries = std::fs::read_dir(dir).map_err(|e| SpecError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let mut spec_paths: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("toml") {
                spec_paths.push(p);
            }
        }
        // Parse all files first (collect parsed specs + their paths).
        let mut parsed: Vec<(PathBuf, PredicateSpec)> = Vec::new();
        for path in spec_paths {
            let content = std::fs::read_to_string(&path).map_err(|e| SpecError::Io {
                path: path.clone(),
                source: e,
            })?;
            let spec = parse_spec_str(&content, &path)?;
            validate_reverse_map(&spec)?;
            parsed.push((path, spec));
        }
        // Topological insert: keep retrying until no more progress can be made.
        // Detect failure when no spec made progress in a full pass.
        let mut g = self.inner.write().unwrap();
        loop {
            let before = g.specs.len();
            parsed.retain(|(path, spec)| {
                let parent_ok = match &spec.parent_predicate {
                    None => true,
                    Some(p) => g.specs.contains_key(p),
                };
                if !parent_ok {
                    return true; // keep for next pass
                }
                match self.install_locked(&mut g, path, spec.clone()) {
                    Ok(()) => false, // installed; drop from pending
                    Err(_) => true,  // keep for next pass (e.g., transient)
                }
            });
            if parsed.is_empty() {
                break;
            }
            if g.specs.len() == before {
                // No progress: parent links cannot be resolved.
                if let Some((_, spec)) = parsed.first() {
                    let parent = spec.parent_predicate.clone().unwrap_or_default();
                    return Err(SpecError::UnknownParent {
                        predicate: spec.name.clone(),
                        parent,
                    });
                }
                break;
            }
        }
        Ok(())
    }

    /// Watch `dir` for `*.toml` create/modify/remove events and update the
    /// registry in place. The returned `WatchHandle` must be kept alive
    /// for the watch to stay active.
    pub fn watch_dir(&self, dir: &Path) -> Result<WatchHandle, SpecError> {
        use notify::Watcher;
        let inner = Arc::clone(&self.inner);
        let mut watcher = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => handle_event(&inner, &event),
                Err(e) => eprintln!("[ffs-core::predicate] watcher error: {e}"),
            },
        )
        .map_err(|e| SpecError::Watcher(e.to_string()))?;
        watcher
            .watch(dir, notify::RecursiveMode::NonRecursive)
            .map_err(|e| SpecError::Watcher(e.to_string()))?;
        Ok(WatchHandle { _watcher: watcher })
    }

    pub fn get(&self, name: &str) -> Option<PredicateSpec> {
        let g = self.inner.read().unwrap();
        g.specs.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let g = self.inner.read().unwrap();
        let mut names: Vec<String> = g.specs.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn validate_claim(
        &self,
        predicate: &str,
        claim: &serde_json::Value,
    ) -> Result<(), ValidationError> {
        let g = self.inner.read().unwrap();
        let validator = g
            .validators
            .get(predicate)
            .ok_or_else(|| ValidationError::UnknownPredicate(predicate.into()))?;
        let errors: Vec<String> = validator
            .iter_errors(claim)
            .map(|e| e.to_string())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::SchemaValidation(errors.join("; ")))
        }
    }

    /// Insert a parsed spec into the registry under the existing write lock.
    /// Compiles the effective JSON Schema (with parent inheritance) and
    /// recompiles validators for any specs that inherit from this one.
    fn install_locked(
        &self,
        g: &mut std::sync::RwLockWriteGuard<'_, RegistryInner>,
        path: &Path,
        spec: PredicateSpec,
    ) -> Result<(), SpecError> {
        if let Some(parent) = &spec.parent_predicate
            && !g.specs.contains_key(parent)
        {
            return Err(SpecError::UnknownParent {
                predicate: spec.name.clone(),
                parent: parent.clone(),
            });
        }

        let name = spec.name.clone();
        let effective = effective_schema(&spec, &g.specs);
        let validator =
            compile_validator(&effective).map_err(|e| SpecError::InvalidJsonSchema {
                predicate: name.clone(),
                message: e,
            })?;

        // If this path previously held a different predicate name, clear it.
        if let Some(old_name) = g.paths.get(path).cloned()
            && old_name != name
        {
            g.specs.remove(&old_name);
            g.validators.remove(&old_name);
        }
        g.paths.insert(path.to_path_buf(), name.clone());
        g.specs.insert(name.clone(), spec);
        g.validators.insert(name.clone(), Arc::new(validator));

        // Recompile any child predicates that inherit from this one.
        let dependents: Vec<String> = g
            .specs
            .iter()
            .filter(|(_, s)| s.parent_predicate.as_deref() == Some(name.as_str()))
            .map(|(n, _)| n.clone())
            .collect();
        for child in dependents {
            if let Some(child_spec) = g.specs.get(&child).cloned() {
                let eff = effective_schema(&child_spec, &g.specs);
                if let Ok(v) = compile_validator(&eff) {
                    g.validators.insert(child, Arc::new(v));
                }
            }
        }
        Ok(())
    }
}

/// Handle a single notify event by reloading or unloading the affected
/// predicate spec(s). Errors are logged to stderr and do not propagate
/// (the watcher keeps running).
fn handle_event(inner: &Arc<RwLock<RegistryInner>>, event: &notify::Event) {
    use notify::EventKind;
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in &event.paths {
                if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                    reload_path(inner, path);
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &event.paths {
                unload_path(inner, path);
            }
        }
        _ => {}
    }
}

fn reload_path(inner: &Arc<RwLock<RegistryInner>>, path: &Path) {
    // The file may not exist yet on the first Create event (rare race);
    // be tolerant and retry once after a short delay.
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            std::thread::sleep(std::time::Duration::from_millis(20));
            match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "[ffs-core::predicate] failed to read {}: {e}",
                        path.display()
                    );
                    return;
                }
            }
        }
    };
    let spec = match parse_spec_str(&content, path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[ffs-core::predicate] failed to parse {}: {e}",
                path.display()
            );
            return;
        }
    };
    if let Err(e) = validate_reverse_map(&spec) {
        eprintln!(
            "[ffs-core::predicate] reverse-map validation failed for {}: {e}",
            spec.name
        );
        return;
    }

    let mut g = inner.write().unwrap();
    if let Some(parent) = &spec.parent_predicate
        && !g.specs.contains_key(parent)
    {
        eprintln!(
            "[ffs-core::predicate] '{}' references unknown parent '{}'; not registered",
            spec.name, parent
        );
        return;
    }
    let name = spec.name.clone();
    let effective = effective_schema(&spec, &g.specs);
    let validator = match compile_validator(&effective) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[ffs-core::predicate] invalid schema in '{}': {e}",
                spec.name
            );
            return;
        }
    };

    if let Some(old_name) = g.paths.get(path).cloned()
        && old_name != name
    {
        g.specs.remove(&old_name);
        g.validators.remove(&old_name);
    }
    g.paths.insert(path.to_path_buf(), name.clone());
    g.specs.insert(name.clone(), spec);
    g.validators.insert(name.clone(), Arc::new(validator));

    let dependents: Vec<String> = g
        .specs
        .iter()
        .filter(|(_, s)| s.parent_predicate.as_deref() == Some(name.as_str()))
        .map(|(n, _)| n.clone())
        .collect();
    for child in dependents {
        if let Some(child_spec) = g.specs.get(&child).cloned() {
            let eff = effective_schema(&child_spec, &g.specs);
            if let Ok(v) = compile_validator(&eff) {
                g.validators.insert(child, Arc::new(v));
            }
        }
    }
}

fn unload_path(inner: &Arc<RwLock<RegistryInner>>, path: &Path) {
    let mut g = inner.write().unwrap();
    if let Some(name) = g.paths.remove(path) {
        g.specs.remove(&name);
        g.validators.remove(&name);
        // Children of the removed predicate are now invalid; drop their
        // validators so subsequent validate_claim calls fail with a clear
        // UnknownPredicate-shaped error chain.
        let orphans: Vec<String> = g
            .specs
            .iter()
            .filter(|(_, s)| s.parent_predicate.as_deref() == Some(name.as_str()))
            .map(|(n, _)| n.clone())
            .collect();
        for orphan in orphans {
            g.validators.remove(&orphan);
        }
    }
}
