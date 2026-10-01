//! The single place where a transport backend is chosen for a target (C-T7,
//! 005 T002): the SSH family by the configured backend, the container family by
//! its resolved runtime. Login, `xxh status` and `xxh clean` all go through it, so
//! nothing downstream knows which backend it talks to (Принцип III).

use xxh_config::{Effective, TransportBackend};
use xxh_core::session::{Progress, SessionError};
use xxh_transport::{
    ContainerCliTransport, ResolvedTarget, RuntimeSelector, RusshTransport, SshCliTransport,
    Transport,
};

/// Pick the backend for `target`. For a container the runtime is resolved here
/// (auto-order docker → podman) and reported, so the backend only verifies it
/// (C-A3/C-C15); the returned target carries the explicit choice.
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
    }
}
