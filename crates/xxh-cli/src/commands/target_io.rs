//! The single place where a transport backend is chosen for a target (C-T7,
//! 005 T002): the SSH family by the configured backend, the container family by
//! its resolved runtime, a Kubernetes pod by `kubectl` (016 T007). Login,
//! `xxh status`, `xxh clean` and `xxh doctor` all go through it, so nothing
//! downstream knows which backend it talks to (Принцип III).

use xxh_config::{Effective, TransportBackend};
use xxh_core::session::{Progress, SessionError};
use xxh_transport::{
    ContainerCliTransport, KubectlTransport, ResolvedTarget, RuntimeSelector, RusshTransport,
    SshCliTransport, Transport,
};

/// Pick the backend for `target`. For a container the runtime is resolved here
/// (auto-order docker → podman) and reported, so the backend only verifies it
/// (C-A3/C-C15); for a pod without a named container the default container is
/// resolved and reported the same way (C-K8S8). The returned target carries the
/// explicit choice.
pub async fn open_transport(
    target: ResolvedTarget,
    eff: &Effective,
    progress: Progress<'_>,
) -> Result<(Box<dyn Transport>, ResolvedTarget), SessionError> {
    match target {
        ResolvedTarget::Ssh(_) => {
            let t: Box<dyn Transport> = match eff.transport {
                TransportBackend::Ssh => Box::new(SshCliTransport::new()?),
                TransportBackend::Russh => Box::new(RusshTransport::new()),
            };
            Ok((t, target))
        }
        ResolvedTarget::Container(mut ct) => {
            let rt = xxh_transport::resolve_runtime(ct.runtime, ct.connect_timeout_s).await?;
            progress(&format!("runtime {rt}"));
            ct.runtime = RuntimeSelector::Explicit(rt);
            Ok((
                Box::new(ContainerCliTransport::new()),
                ResolvedTarget::Container(ct),
            ))
        }
        ResolvedTarget::Kube(mut kt) => {
            if kt.container.is_none() {
                let container = xxh_transport::resolve_pod_container(&kt).await?;
                progress(&format!("container {container}"));
                kt.container = Some(container);
            }
            Ok((Box::new(KubectlTransport::new()), ResolvedTarget::Kube(kt)))
        }
    }
}
