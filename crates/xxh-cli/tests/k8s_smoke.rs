//! Integration test (016 T008/T009, US1/US2): a Kubernetes pod as a target, over
//! the **real `KubectlTransport`** and the **real `xxh` binary** against a live
//! cluster — no sshd in the pod, no change to its spec.
//!
//! - a session in the annotated default container delivers the environment and
//!   leaves the container clean (SC-001, SC-002);
//! - one command keeps its exit code and separate, byte-exact streams — kubectl's
//!   own exit note never reaches stderr (SC-003, 004 C-X1);
//! - `--keep`, `xxh status`, `xxh clean` work on a pod;
//! - a missing pod / container is a transport error (code 10), `-l` a config
//!   error (code 40) — and the pod spec is never touched (FR-004).
//!
//! Cluster: `XXH_TEST_KUBE_CONTEXT` or the current kubeconfig context; skips
//! (passes) with a message when none is reachable (C-DT8).

mod common;

use std::process::Command;

use common::{KubeFixture, eff, kube_available, kube_context, run_with_stdin};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::{KubeTarget, KubectlTransport, ResolvedTarget};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn pod_sessions_match_container_sessions_and_leave_the_pod_clean() {
    let Some(ctx) = kube_context().filter(|c| kube_available(c)) else {
        eprintln!("skipping: no reachable Kubernetes cluster (set XXH_TEST_KUBE_CONTEXT)");
        return;
    };
    let fx = KubeFixture::boot(&ctx);
    let spec_before = fx.spec_state();

    // 1) Session over the transport, no container named: the annotated default
    //    `side` is used.
    let target = ResolvedTarget::Kube(KubeTarget {
        namespace: Some(fx.namespace.clone()),
        context: Some(ctx.clone()),
        ..KubeTarget::new(fx.pod.clone())
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let code = rt.block_on(async {
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut session = Session::establish(
            KubectlTransport::new(),
            &target,
            &eff("sh", CleanupMode::Ephemeral),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish pod session");
        let code = session
            .run_command("test \"$XXH_SESSION\" = 1 && test \"$HOSTNAME\" = target")
            .await
            .expect("run command in pod env");
        session.finish().await.expect("finish");
        code
    });
    assert_eq!(code, 0, "env should be sourced in the pod (XXH_SESSION=1)");
    assert_eq!(
        fx.cleanliness("side"),
        "CLEAN",
        "default container must be clean"
    );

    // 2) The real binary, one command in a named container.
    let xxh = |args: &[&str]| {
        let mut c = Command::new(XXH);
        c.args(["--shell", "sh", "--kube-context", &ctx]).args(args);
        let (code, out, err) = run_with_stdin(c, b"");
        (
            code,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    };
    let app = fx.address(Some("app"));
    let (code, out, err) = xxh(&[
        &app,
        "--",
        "sh",
        "-c",
        "echo $XXH_SESSION; echo oops >&2; exit 7",
    ]);
    assert_eq!(code, Some(7), "stderr: {err}");
    assert_eq!(out, "1\n");
    assert_eq!(
        err, "oops\n",
        "only the command's own stderr, no kubectl note"
    );
    assert_eq!(fx.cleanliness("app"), "CLEAN");

    let (code, out, err) = xxh(&[&app, "-c", "printf out; printf err >&2"]);
    assert_eq!((code, out.as_str(), err.as_str()), (Some(0), "out", "err"));

    // The default container is reported when none is named (C-K8S8).
    let (code, _, err) = xxh(&["-v", &fx.address(None), "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(err.contains("container side"), "stderr: {err}");

    // 3) --keep, status, clean.
    let (code, _, err) = xxh(&["--keep", &app, "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(fx.cleanliness("app"), "DIRTY", "--keep must keep the env");
    let (code, out, err) = xxh(&["status", &app]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(!out.trim().is_empty(), "status must report the kept env");
    let (code, _, err) = xxh(&["clean", &app]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(fx.cleanliness("app"), "CLEAN", "clean must remove the env");

    // 4) Errors: their class, and nothing written to the pod.
    let (code, _, err) = xxh(&[&format!("k8s:{}/no-such-pod", fx.namespace), "--", "true"]);
    assert_eq!(code, Some(10), "stderr: {err}");
    assert!(err.contains("pod not found"), "stderr: {err}");
    let (code, _, err) = xxh(&[&fx.address(Some("db")), "--", "true"]);
    assert_eq!(code, Some(10), "stderr: {err}");
    assert!(err.contains("app, side"), "stderr: {err}");
    let (code, _, err) = xxh(&["-l", "root", &app, "--", "true"]);
    assert_eq!(code, Some(40), "stderr: {err}");

    assert_eq!(fx.cleanliness("app"), "CLEAN");
    assert_eq!(fx.cleanliness("side"), "CLEAN");
    assert_eq!(fx.spec_state(), spec_before, "the pod spec must not change");
}
