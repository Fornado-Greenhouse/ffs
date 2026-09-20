//! Entity-resolution configuration (ADR-030): Fellegi-Sunter style
//! agreement weights per compared field plus the two thresholds that
//! split scores into `existing` / `ambiguous` / `new`.
//!
//! This module only loads and validates the file. The resolver that
//! consumes it is task_45 (daemon side). Shipped in
//! `starter/config/resolution.toml` and installed to
//! `$FFS_DATA_DIR/config/resolution.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolutionConfigError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("toml parse error in resolution config: {0}")]
    Toml(#[from] toml::de::Error),
    #[error(
        "thresholds must satisfy review_floor < auto_link (got review_floor={review_floor}, auto_link={auto_link})"
    )]
    ThresholdOrder { review_floor: f64, auto_link: f64 },
    #[error("resolution config declares no weights")]
    NoWeights,
}

/// Log-odds contribution of one compared field.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldWeight {
    pub agree: f64,
    pub disagree: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    /// Scores at or above this link to the best candidate.
    pub auto_link: f64,
    /// Scores below this mint a new entity; between the two is review.
    pub review_floor: f64,
    /// Two candidates whose scores are within this margin of each
    /// other are ambiguous even when the best clears `auto_link`.
    #[serde(default = "default_margin")]
    pub margin: f64,
}

fn default_margin() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

/// How candidates are blocked before scoring (ADR-030): only entities
/// in the same path family, keyed by the full normalized name or by
/// the surname alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockingConfig {
    #[serde(default = "default_true")]
    pub same_family: bool,
    #[serde(default)]
    pub key: BlockingKey,
}

impl Default for BlockingConfig {
    fn default() -> Self {
        Self {
            same_family: true,
            key: BlockingKey::FullName,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockingKey {
    #[default]
    FullName,
    Surname,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionConfig {
    pub thresholds: Thresholds,
    /// Keyed by compared field name (`display_name`, `alias`, ...).
    /// Defaults to empty so a missing table is reported as `NoWeights`
    /// rather than a parse error.
    #[serde(default)]
    pub weights: BTreeMap<String, FieldWeight>,
    #[serde(default)]
    pub blocking: BlockingConfig,
}

impl ResolutionConfig {
    pub fn from_toml_str(content: &str) -> Result<Self, ResolutionConfigError> {
        let cfg: ResolutionConfig = toml::from_str(content)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self, ResolutionConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ResolutionConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::from_toml_str(&content)
    }

    fn validate(&self) -> Result<(), ResolutionConfigError> {
        if self.thresholds.review_floor >= self.thresholds.auto_link {
            return Err(ResolutionConfigError::ThresholdOrder {
                review_floor: self.thresholds.review_floor,
                auto_link: self.thresholds.auto_link,
            });
        }
        if self.weights.is_empty() {
            return Err(ResolutionConfigError::NoWeights);
        }
        Ok(())
    }

    /// Weight for a field, if declared.
    pub fn weight(&self, field: &str) -> Option<FieldWeight> {
        self.weights.get(field).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../starter/config/resolution.toml")
    }

    #[test]
    fn starter_resolution_config_loads_and_thresholds_are_ordered() {
        let cfg = ResolutionConfig::load(&starter_path()).expect("starter config loads");
        assert!(cfg.thresholds.review_floor < cfg.thresholds.auto_link);
        for field in [
            "display_name",
            "alias",
            "organization",
            "role",
            "location",
            "surname",
        ] {
            assert!(cfg.weight(field).is_some(), "missing weight for {field}");
        }
        // The ADR-030 hints: organization agreement strong, surname weak.
        assert!(cfg.weight("organization").unwrap().agree > cfg.weight("surname").unwrap().agree);
    }

    #[test]
    fn margin_and_blocking_default_when_absent_and_load_when_present() {
        let minimal = "[thresholds]\nauto_link = 5.0\nreview_floor = 1.0\n[weights.x]\nagree = 1.0\ndisagree = 0.0\n";
        let cfg = ResolutionConfig::from_toml_str(minimal).unwrap();
        assert_eq!(cfg.thresholds.margin, 1.0);
        assert_eq!(cfg.blocking, BlockingConfig::default());
        let full = "[thresholds]\nauto_link = 5.0\nreview_floor = 1.0\nmargin = 0.5\n[blocking]\nsame_family = false\nkey = \"surname\"\n[weights.x]\nagree = 1.0\ndisagree = 0.0\n";
        let cfg = ResolutionConfig::from_toml_str(full).unwrap();
        assert_eq!(cfg.thresholds.margin, 0.5);
        assert!(!cfg.blocking.same_family);
        assert_eq!(cfg.blocking.key, BlockingKey::Surname);
        let starter = ResolutionConfig::load(&starter_path()).unwrap();
        assert_eq!(starter.blocking.key, BlockingKey::FullName);
        assert!(starter.thresholds.margin > 0.0);
    }

    #[test]
    fn inverted_thresholds_are_rejected() {
        let bad = r#"
[thresholds]
auto_link = 1.0
review_floor = 5.0
[weights.display_name]
agree = 1.0
disagree = -1.0
"#;
        let err = ResolutionConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ResolutionConfigError::ThresholdOrder { .. }));
    }

    #[test]
    fn empty_weights_are_rejected_and_unknown_keys_fail_loudly() {
        let no_weights = "[thresholds]\nauto_link = 5.0\nreview_floor = 1.0\n";
        assert!(matches!(
            ResolutionConfig::from_toml_str(no_weights).unwrap_err(),
            ResolutionConfigError::NoWeights
        ));
        let unknown = "[thresholds]\nauto_link = 5.0\nreview_floor = 1.0\nbogus = 1\n[weights.x]\nagree = 1.0\ndisagree = 0.0\n";
        assert!(matches!(
            ResolutionConfig::from_toml_str(unknown).unwrap_err(),
            ResolutionConfigError::Toml(_)
        ));
    }
}
