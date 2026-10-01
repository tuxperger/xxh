//! ⭐ Integration test (003 T018/T021/T023, US2–US4, feature `nix-source`): the
//! flake source against a **real `nix`** and a real flake repository — pinning and
//! explicit update, ready-made plugin outputs, and client-side rejection of
//! artefacts no target could run. No target is involved (nothing is delivered),
//! so only Nix is required; skips (passes) when it is absent.

#![cfg(feature = "nix-source")]

mod common;

use std::path::Path;

use common::{git_in, make_test_flake, nix_available};
use xxh_plugins::registry::Registry;
use xxh_plugins::source::SourceSpec;

fn revision(reg: &Registry, name: &str) -> Option<String> {
    match reg.entry(name).unwrap().source {
        SourceSpec::Flake { revision, .. } => revision,
        other => panic!("expected a flake source, got {other:?}"),
    }
}

fn head(dir: &Path) -> String {
    git_in(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn cache_entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn flake_source_pins_updates_and_rejects() {
    if !nix_available() {
        eprintln!("skipping: nix with flakes not available on client");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch =
        std::env::temp_dir().join(format!("xxh-flakeerr-{}-{nanos:x}", std::process::id()));
    let flake_dir = scratch.join("flake");
    let nix_cache = scratch.join("nix-cache");
    std::fs::create_dir_all(&nix_cache).unwrap();
    // SAFETY: single-test binary; set before any provider use.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("XXH_NIX_CACHE_DIR", &nix_cache);
    }
    make_test_flake(&flake_dir);
    let flake =
        |attr: &str| SourceSpec::parse(&format!("flake:{}#{attr}", flake_dir.display())).unwrap();

    let registry = Registry::open(scratch.join("registry"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // ── US2: the revision is pinned at install time (§FR-014) ────────────
        let first = head(&flake_dir);
        registry.install(&flake("tool")).await.expect("install");
        assert_eq!(revision(&registry, "tool").as_deref(), Some(first.as_str()));
        let hash = registry.entry("tool").unwrap().hash;
        let cached = cache_entries(&nix_cache);

        // The source moves on; the installed plugin does not (§FR-015) — and
        // re-installing the recorded spec repackages nothing (§FR-019).
        git_in(&flake_dir, &["commit", "-q", "--allow-empty", "-m", "next"]);
        let second = head(&flake_dir);
        assert_ne!(first, second);
        let recorded = registry.entry("tool").unwrap().source;
        registry
            .install(&recorded)
            .await
            .expect("re-install pinned");
        assert_eq!(revision(&registry, "tool").as_deref(), Some(first.as_str()));
        assert_eq!(registry.entry("tool").unwrap().hash, hash);
        assert_eq!(cache_entries(&nix_cache), cached, "no repackaging");

        // Only an explicit update re-resolves the reference (§FR-016).
        registry.update("tool").await.expect("update");
        assert_eq!(
            revision(&registry, "tool").as_deref(),
            Some(second.as_str())
        );
        assert_eq!(
            registry.entry("tool").unwrap().hash,
            hash,
            "same output ⇒ same content address"
        );

        // ── US3: a ready-made plugin output is used as is (§FR-005) ──────────
        let m = registry
            .install(&flake("plugin"))
            .await
            .expect("plugin output");
        assert_eq!(
            (m.name.as_str(), m.version.to_string()),
            ("demo", "2.0.0".into())
        );
        assert!(
            registry
                .package_dir("demo")
                .unwrap()
                .join("env.sh")
                .is_file()
        );
        // Its name comes from its manifest: --name does not apply (C-F10).
        let mut named = flake("plugin");
        if let SourceSpec::Flake { name, .. } = &mut named {
            *name = Some("other".into());
        }
        assert!(registry.install(&named).await.is_err());
        assert!(!registry.list().unwrap().contains_key("other"));

        // A wrapped program does take a chosen name (§FR-008).
        let mut named = flake("tool");
        if let SourceSpec::Flake { name, .. } = &mut named {
            *name = Some("greeter".into());
        }
        let m = registry.install(&named).await.expect("named program");
        assert_eq!(m.name, "greeter");
        assert_eq!(
            m.targets.len(),
            1,
            "built platform is recorded: {:?}",
            m.targets
        );
        assert!(m.targets[0].starts_with("linux/"), "got {:?}", m.targets);

        // ── US4: unusable artefacts are rejected on the client (§FR-010/022) ─
        let before = (registry.list().unwrap().len(), cache_entries(&nix_cache));
        let err = registry
            .install(&flake("dyn"))
            .await
            .expect_err("a dynamically linked program must be rejected")
            .to_string();
        assert!(err.contains("NotSelfContained"), "got: {err}");
        assert!(
            err.contains("bin/hello"),
            "the offending file is named: {err}"
        );
        let err = registry
            .install(&flake("no-such-output"))
            .await
            .expect_err("a missing output must fail")
            .to_string();
        assert!(err.contains("BuildFailed"), "got: {err}");
        assert_eq!(
            (registry.list().unwrap().len(), cache_entries(&nix_cache)),
            before,
            "failed installs leave no trace in the registry or the cache"
        );

        // ── US2: a dirty tree has no revision to pin (§FR-018) ───────────────
        let mut text = std::fs::read_to_string(flake_dir.join("flake.nix")).unwrap();
        text.push_str("# local edit\n");
        std::fs::write(flake_dir.join("flake.nix"), text).unwrap();
        registry
            .update("tool")
            .await
            .expect("update from a dirty tree");
        assert_eq!(revision(&registry, "tool"), None, "dirty tree ⇒ unpinned");
    });

    let _ = std::fs::remove_dir_all(&scratch);
}
