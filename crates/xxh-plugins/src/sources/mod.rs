//! Package-source provider implementations (contracts/plugin-source-trait.md).

pub mod git;
pub mod local;

// ⭐ Optional Nix provider is compiled only behind the `nix-source` feature so its
// absence cannot affect the rest of the tool (Принцип IX).
#[cfg(feature = "nix-source")]
pub mod nix;
