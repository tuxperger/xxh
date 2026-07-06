//! JSON Schema export for the canonical config (T054, Принцип XI, C-CM4).
//!
//! The Rust types in this crate are the **single source of truth**; the exported
//! schema (`nix/config-schema.json`) is what the declarative Nix modules and the
//! round-trip test check themselves against, so the module options cannot drift
//! from the parser silently.
//!
//! Regenerate with: `XXH_REGEN_SCHEMA=1 cargo test -p xxh-config schema`.

use crate::Config;

/// The machine-readable JSON Schema of [`Config`], pretty-printed.
pub fn config_schema_json() -> String {
    let schema = schemars::schema_for!(Config);
    serde_json::to_string_pretty(&schema).expect("schema always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn schema_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../nix/config-schema.json")
    }

    /// Anti-drift gate: the committed schema must match the types (C-CM4).
    /// Set `XXH_REGEN_SCHEMA=1` to (re)generate the file instead of failing.
    #[test]
    fn committed_schema_matches_config_types() {
        let current = config_schema_json();
        let path = schema_path();
        if std::env::var_os("XXH_REGEN_SCHEMA").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &current).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "{} is missing — regenerate with XXH_REGEN_SCHEMA=1 cargo test -p xxh-config",
                path.display()
            )
        });
        assert_eq!(
            committed, current,
            "nix/config-schema.json is stale — regenerate with \
             XXH_REGEN_SCHEMA=1 cargo test -p xxh-config"
        );
    }
}
