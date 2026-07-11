//! Integration test (002 T014, US1): full session over the **real
//! `ContainerCliTransport`** against a live container — no sshd, no keys (C-DT2).
//! connect → detect → deploy env → run a command in the env → exit → assert the
//! container is clean (`~/.xxh` gone) and the image is unmodified (`diff` empty of
//! xxh artefacts) — the 001 scenario at parity plus the image-immutability assert
//! required for the container path (C-DT1/C-DT3, §SC-002).
//!
//! Requires a container runtime (docker by default; `XXH_TEST_RUNTIME=podman`).
//! Skips (passes) with an explicit message when none is available (C-DT8).

mod common;

use common::{ContainerFixture, container_base_image, eff, runtime_available, test_runtime};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::ContainerCliTransport;

#[test]
fn full_session_over_container_leaves_it_clean() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }

    let fx = ContainerFixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (code, clean, diff) = rt.block_on(async {
        // MVP: the container's own `sh`; real shells are packaged plugins (T020).
        let eff = eff("sh", CleanupMode::Ephemeral);
        let env = vec![minimal_env_component("gz").unwrap()];

        let mut session = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish container session");

        // The delivered env.sh sets XXH_SESSION=1: prove config delivery end-to-end.
        let code = session
            .run_command("test \"$XXH_SESSION\" = 1 && echo OK")
            .await
            .expect("run command in container env");
        assert_eq!(code, 0, "env should be sourced (XXH_SESSION=1)");
        session.finish().await.expect("finish");

        // Cleanliness (Принцип VIII) + image immutability (C-DT3): both out-of-band.
        (code, fx.cleanliness(), fx.diff())
    });

    assert_eq!(code, 0);
    assert_eq!(
        clean,
        "CLEAN",
        "the xxh root must be gone after an ephemeral container session (image {})",
        container_base_image()
    );
    assert!(
        !diff.contains(".xxh"),
        "the container filesystem must carry no xxh artefacts after exit; diff:\n{diff}"
    );
}

/// US2 mandatory cell (C-C7): the same entry works on a bare musl/BusyBox image
/// that lacks a full user shell. Platform detection inside the container must
/// report `musl`, and the delivered environment must still function even though
/// the image has no `zsh`.
#[test]
fn alpine_musl_image_has_no_user_shell_yet_env_works() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    if container_base_image() != "alpine:3.20" {
        eprintln!("skipping: XXH_TEST_IMAGE is not alpine (musl cell)");
        return;
    }

    let fx = ContainerFixture::boot();
    // Precondition: the image really is the bare musl case — no zsh present.
    let has_zsh = fx.exec("command -v zsh >/dev/null 2>&1 && echo YES || echo NO");
    assert_eq!(
        has_zsh.trim(),
        "NO",
        "alpine base must ship no user shell (zsh) — that is the point of this cell"
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (libc, code) = rt.block_on(async {
        let eff = eff("sh", CleanupMode::Ephemeral);
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut session = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish on alpine musl container");
        let libc = session.platform().libc_str().to_string();
        let code = session
            .run_command("test \"$XXH_SESSION\" = 1 && echo OK")
            .await
            .expect("run command");
        session.finish().await.expect("finish");
        (libc, code)
    });

    assert_eq!(
        libc, "musl",
        "platform detection inside alpine must report musl"
    );
    assert_eq!(
        code, 0,
        "the delivered environment must work on the bare image"
    );
}
