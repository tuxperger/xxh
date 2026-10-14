//! Kubernetes transport backend over the `kubectl` CLI (016 T003, research R1,
//! contracts/k8s-target.md).
//!
//! `connect` checks the pod with one bounded `kubectl get pod` (phase, containers,
//! default container) and pins the container; afterwards `kubectl exec` is the
//! ONLY data channel (C-K8S10 — no `cp`, `debug`, `attach` or writes to the pod
//! object). The user's kubeconfig, auth plugins and contexts are honoured by
//! inheriting the environment untouched (FR-005).
//!
//! Never shown or logged: `kubectl`'s own stderr (it names the API server and
//! may echo auth details) and session env values (C-K8S9/C-K8S11) — errors use
//! our own wording.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::container_backend::{check_sh_present, run_bounded, which};
use crate::{
    AuthPolicy, ExecOutput, KubeTarget, PtySpec, ResolvedTarget, Transport, TransportError,
};

/// The `kubectl` binary, looked up on `PATH`.
const KUBECTL: &str = "kubectl";

/// `kubectl get pod -o jsonpath` template: `phase|default-container|c1 c2 …`
/// (research R3). A missing annotation renders as an empty field.
const POD_INFO_TEMPLATE: &str = "jsonpath={.status.phase}{\"|\"}\
{.metadata.annotations.kubectl\\.kubernetes\\.io/default-container}{\"|\"}\
{.spec.containers[*].name}";

/// The pod fields `connect` needs (data-model PodInfo).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PodInfo {
    phase: String,
    default_container: Option<String>,
    containers: Vec<String>,
}

impl PodInfo {
    fn parse(out: &str) -> Option<PodInfo> {
        let mut parts = out.trim().splitn(3, '|');
        let phase = parts.next()?.trim().to_string();
        let default = parts.next()?.trim();
        let containers = parts
            .next()?
            .split_whitespace()
            .map(str::to_string)
            .collect();
        Some(PodInfo {
            phase,
            default_container: (!default.is_empty()).then(|| default.to_string()),
            containers,
        })
    }

    /// The container the session runs in (C-K8S8): the requested one if the pod
    /// has it, else the annotated default, else the first one.
    fn pick(&self, requested: Option<&str>, pod: &str) -> Result<String, TransportError> {
        let listed = || self.containers.join(", ");
        match requested {
            Some(c) if self.containers.iter().any(|n| n == c) => Ok(c.to_string()),
            Some(c) => Err(TransportError::Connect(format!(
                "pod {pod} has no container `{c}` (containers: {})",
                listed()
            ))),
            None => self
                .default_container
                .clone()
                .filter(|d| self.containers.contains(d))
                .or_else(|| self.containers.first().cloned())
                .ok_or_else(|| TransportError::Connect(format!("pod {pod} has no containers"))),
        }
    }
}

/// Map a failed `kubectl get pod` to a transport error class without echoing
/// its stderr (C-K8S7/C-K8S9).
fn classify_get_failure(stderr: &str, pod: &str, context: Option<&str>) -> TransportError {
    let s = stderr.to_lowercase();
    if s.contains("(notfound)") || s.contains(" not found") {
        TransportError::Connect(format!("pod not found: {pod}"))
    } else if s.contains("forbidden")
        || s.contains("unauthorized")
        || s.contains("must be logged in")
        || s.contains("provide credentials")
    {
        TransportError::Auth(format!(
            "no access to pod {pod} (check your kubeconfig credentials and RBAC)"
        ))
    } else if let Some(ctx) =
        context.filter(|_| s.contains("context") && s.contains("does not exist"))
    {
        TransportError::Connect(format!("kubeconfig has no context `{ctx}`"))
    } else {
        TransportError::Connect("the Kubernetes cluster is not reachable".into())
    }
}

/// Quote `s` for POSIX `sh` (single quotes, `'` as `'\''`).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The PTY session script: `kubectl exec` has no `-e`, so the session's
/// variables are exported inside `sh -c` before the shell starts (R4, C-K8S11).
fn pty_script(spec: &PtySpec) -> String {
    let mut script = format!("export TERM={};", sh_quote(&spec.term));
    for (k, v) in &spec.env {
        script.push_str(&format!(" export {k}={};", sh_quote(v)));
    }
    script.push_str(&format!(" exec {}", spec.shell_cmd));
    script
}

/// The line `kubectl exec` adds to its stderr when the command exits non-zero —
/// `-q` does not silence it. It is not the command's output (004 C-X1).
const EXIT_NOTE: &[u8] = b"command terminated with exit code ";

/// Drops kubectl's exit note from a stderr stream without buffering it whole:
/// only a line that is still a prefix of `EXIT_NOTE <digits>\n` is held back,
/// everything else passes through at once. At the end a held, complete note is
/// dropped iff its number is the command's exit code.
#[derive(Default)]
struct ExitNoteFilter {
    /// Bytes of the current line held back as a possible note.
    held: Vec<u8>,
    /// The current line already proved not to be a note.
    passing: bool,
}

impl ExitNoteFilter {
    /// Whether `line` can still become (or is) a note; `Some(true)` — complete.
    fn note_state(line: &[u8]) -> Option<bool> {
        if line.len() <= EXIT_NOTE.len() {
            return EXIT_NOTE.starts_with(line).then_some(false);
        }
        let rest = line.strip_prefix(EXIT_NOTE)?;
        let (digits, complete) = match rest.strip_suffix(b"\n") {
            Some(d) => (d, true),
            None => (rest, false),
        };
        if !digits.iter().all(u8::is_ascii_digit) || (complete && digits.is_empty()) {
            return None;
        }
        Some(complete)
    }

    /// Feed a chunk; returns the bytes that can be emitted now.
    fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(chunk.len());
        for &b in chunk {
            if self.passing {
                out.push(b);
                self.passing = b != b'\n';
                continue;
            }
            // A complete note followed by more output was not kubectl's.
            if Self::note_state(&self.held) == Some(true) {
                out.append(&mut self.held);
            }
            self.held.push(b);
            if Self::note_state(&self.held).is_none() {
                out.append(&mut self.held);
                self.passing = b != b'\n';
            }
        }
        out
    }

    /// End of stream: what is left to emit, without the note for `code`.
    fn finish(self, code: i32) -> Vec<u8> {
        let note = [EXIT_NOTE, format!("{code}\n").as_bytes()].concat();
        if code != 0 && self.held == note {
            Vec::new()
        } else {
            self.held
        }
    }

    fn apply(stderr: &[u8], code: i32) -> Vec<u8> {
        let mut f = Self::default();
        let mut out = f.feed(stderr);
        out.extend(f.finish(code));
        out
    }
}

/// A captured exec result with kubectl's exit note removed.
fn captured(out: std::process::Output) -> ExecOutput {
    let exit_code = out.status.code().unwrap_or(-1);
    ExecOutput {
        exit_code,
        stdout: out.stdout,
        stderr: ExitNoteFilter::apply(&out.stderr, exit_code),
    }
}

/// kubectl's stderr being copied to the client's stderr through the filter.
pub(crate) struct StderrPump(tokio::task::JoinHandle<ExitNoteFilter>);

impl StderrPump {
    /// Wait for the stream to end and emit what was held back, minus the note.
    async fn finish(self, code: i32) {
        if let Ok(filter) = self.0.await {
            write_stderr(&filter.finish(code));
        }
    }
}

fn forward_stderr(mut src: tokio::process::ChildStderr) -> StderrPump {
    StderrPump(tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut filter = ExitNoteFilter::default();
        let mut buf = [0u8; 8192];
        while let Ok(n) = src.read(&mut buf).await {
            if n == 0 {
                break;
            }
            write_stderr(&filter.feed(&buf[..n]));
        }
        filter
    }))
}

fn write_stderr(bytes: &[u8]) {
    use std::io::Write;
    if !bytes.is_empty() {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(bytes);
        let _ = err.flush();
    }
}

/// The pod a connected transport talks to.
#[derive(Debug, Clone)]
struct Pinned {
    pod: String,
    namespace: Option<String>,
    container: String,
    context: Option<String>,
}

/// Transport over `kubectl exec`. One instance serves one pod; `connect` checks
/// the pod and pins the container before anything else happens.
pub struct KubectlTransport {
    /// `kubectl` on `PATH`, or a stand-in path in tests.
    bin: std::path::PathBuf,
    pinned: Option<Pinned>,
}

impl KubectlTransport {
    pub fn new() -> Self {
        Self {
            bin: KUBECTL.into(),
            pinned: None,
        }
    }

    #[cfg(test)]
    fn with_binary(bin: std::path::PathBuf) -> Self {
        Self { bin, pinned: None }
    }

    /// The container `connect` chose, for the progress line (C-K8S8).
    pub fn container(&self) -> Option<&str> {
        self.pinned.as_ref().map(|p| p.container.as_str())
    }

    fn pinned(&self) -> Result<&Pinned, TransportError> {
        self.pinned
            .as_ref()
            .ok_or_else(|| TransportError::Channel("not connected".into()))
    }

    /// `kubectl exec -q [-i] [-t] <pod> -c <container> -- sh -c <cmd>`; `-c` is
    /// always explicit so kubectl never prints its "Defaulted container" note,
    /// and `-q` keeps its own messages off the command's stderr (C-K8S10, 004 C-X1).
    fn exec_cmd(&self, interactive: bool, tty: bool, cmd: &str) -> Result<Command, TransportError> {
        let p = self.pinned()?;
        let mut c = kubectl(&self.bin, p.context.as_deref(), p.namespace.as_deref());
        c.args(["exec", "-q"]);
        if interactive {
            c.arg("-i");
        }
        if tty {
            c.arg("-t");
        }
        c.args([
            p.pod.as_str(),
            "-c",
            p.container.as_str(),
            "--",
            "sh",
            "-c",
            cmd,
        ]);
        Ok(c)
    }
}

impl Default for KubectlTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// `kubectl [--context C] [-n N]` — global flags first, subcommand after.
fn kubectl(bin: &Path, context: Option<&str>, namespace: Option<&str>) -> Command {
    let mut c = Command::new(bin);
    if let Some(ctx) = context {
        c.args(["--context", ctx]);
    }
    if let Some(ns) = namespace {
        c.args(["-n", ns]);
    }
    c
}

/// The container a session in `target` runs in, after checking that kubectl is
/// installed, the pod exists, is reachable and running (C-K8S6..C-K8S8). The CLI
/// calls it before connecting when no container was named, to report the choice.
pub async fn resolve_pod_container(target: &KubeTarget) -> Result<String, TransportError> {
    inspect_pod(Path::new(KUBECTL), target).await
}

async fn inspect_pod(bin: &Path, target: &KubeTarget) -> Result<String, TransportError> {
    if target.pod.is_empty() {
        return Err(TransportError::Connect("empty pod name".into()));
    }
    let missing = if bin.components().count() == 1 {
        which(KUBECTL).is_none()
    } else {
        !bin.is_file()
    };
    if missing {
        return Err(TransportError::BackendUnavailable(
            "kubectl is not installed (`kubectl` not found on PATH)".into(),
        ));
    }

    let timeout_s = target.connect_timeout_s;
    let mut get = kubectl(bin, target.context.as_deref(), target.namespace.as_deref());
    get.args([
        "get",
        "pod",
        &target.pod,
        "-o",
        POD_INFO_TEMPLATE,
        &format!("--request-timeout={timeout_s}s"),
    ])
    .stdin(Stdio::null());
    let out = run_bounded(&mut get, timeout_s).await?;
    if !out.status.success() {
        return Err(classify_get_failure(
            &String::from_utf8_lossy(&out.stderr),
            &target.pod,
            target.context.as_deref(),
        ));
    }
    let info = PodInfo::parse(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        TransportError::Connect(format!("unexpected pod description for {}", target.pod))
    })?;
    if info.phase != "Running" {
        let phase = if info.phase.is_empty() {
            "unknown"
        } else {
            &info.phase
        };
        return Err(TransportError::Connect(format!(
            "pod {} is not running (phase {phase})",
            target.pod
        )));
    }
    info.pick(target.container.as_deref(), &target.pod)
}

#[async_trait::async_trait]
impl Transport for KubectlTransport {
    async fn connect(
        &mut self,
        target: &ResolvedTarget,
        _auth: &AuthPolicy, // SSH-family only; access = the user's kubeconfig (C-T8)
    ) -> Result<(), TransportError> {
        // Kubernetes family only: anything else fails fast (C-K8S13).
        let ResolvedTarget::Kube(target) = target else {
            return Err(TransportError::BackendUnavailable(
                "kubectl backend serves Kubernetes pod targets only".into(),
            ));
        };
        let container = inspect_pod(&self.bin, target).await?;
        tracing::debug!(
            context = target.context.as_deref().unwrap_or("(current)"),
            namespace = target.namespace.as_deref().unwrap_or("(context)"),
            pod = %target.pod,
            container = %container,
            "pod attach"
        );
        self.pinned = Some(Pinned {
            pod: target.pod.clone(),
            namespace: target.namespace.clone(),
            container,
            context: target.context.clone(),
        });
        Ok(())
    }

    async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError> {
        let mut c = self.exec_cmd(false, false, cmd)?;
        c.stdin(Stdio::null());
        let out = c.output().await?;
        check_sh_present(cmd, &out)?;
        Ok(captured(out))
    }

    async fn upload_stream(
        &mut self,
        remote_cmd: &str,
        data: Vec<u8>,
    ) -> Result<ExecOutput, TransportError> {
        let mut c = self.exec_cmd(true, false, remote_cmd)?;
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&data).await?;
            stdin.shutdown().await?;
        }
        let out = child.wait_with_output().await?;
        check_sh_present(remote_cmd, &out)?;
        Ok(captured(out))
    }

    async fn exec_stream(&mut self, cmd: &str) -> Result<i32, TransportError> {
        // `-i` without `-t`: stdin forwarded, no TTY, streams separate; stdin and
        // stdout are the client's own, stderr passes through the exit-note
        // filter (004 C-X1/C-X2).
        let mut c = self.exec_cmd(true, false, cmd)?;
        c.stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = c.spawn()?;
        let pump = child.stderr.take().map(forward_stderr);
        let status = child.wait().await?;
        let code = crate::exit_code_of(&status);
        if let Some(pump) = pump {
            pump.finish(code).await;
        }
        Ok(code)
    }

    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError> {
        // With `-t` the remote stderr arrives on the terminal; kubectl's own
        // stderr carries only its exit note, which is filtered out.
        let c = self.exec_cmd(true, true, &pty_script(spec))?;
        let (code, pump) =
            crate::pty::run_in_local_pty(c, spec.cols, spec.rows, Some(forward_stderr)).await?;
        if let Some(pump) = pump {
            pump.finish(code).await;
        }
        Ok(code)
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        // exec-model: no persistent connection to tear down.
        self.pinned = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(phase: &str, default: Option<&str>, containers: &[&str]) -> PodInfo {
        PodInfo {
            phase: phase.into(),
            default_container: default.map(str::to_string),
            containers: containers.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn pod_info_is_parsed() {
        assert_eq!(
            PodInfo::parse("Running||app\n").unwrap(),
            info("Running", None, &["app"])
        );
        assert_eq!(
            PodInfo::parse("Running|side|app side").unwrap(),
            info("Running", Some("side"), &["app", "side"])
        );
        assert_eq!(
            PodInfo::parse("Pending||").unwrap(),
            info("Pending", None, &[])
        );
        assert!(PodInfo::parse("garbage").is_none());
    }

    /// Requested > annotated default > first; a missing one lists the pod's (C-K8S8).
    #[test]
    fn container_is_picked() {
        let pod = info("Running", Some("side"), &["app", "side"]);
        assert_eq!(pod.pick(Some("app"), "p").unwrap(), "app");
        assert_eq!(pod.pick(None, "p").unwrap(), "side");
        let plain = info("Running", None, &["app", "side"]);
        assert_eq!(plain.pick(None, "p").unwrap(), "app");
        // A stale annotation naming no container falls back to the first.
        let stale = info("Running", Some("gone"), &["app"]);
        assert_eq!(stale.pick(None, "p").unwrap(), "app");
        let err = pod.pick(Some("db"), "p").unwrap_err().to_string();
        assert!(
            err.contains("no container `db`") && err.contains("app, side"),
            "{err}"
        );
    }

    /// Error classes are distinguishable and never echo kubectl's stderr (C-K8S9).
    #[test]
    fn get_failures_are_classified() {
        let secret = "https://10.0.0.1:6443 token=abc";
        let e = classify_get_failure(
            &format!("Error from server (NotFound): pods \"web\" not found {secret}"),
            "web",
            None,
        );
        assert!(matches!(&e, TransportError::Connect(m) if m == "pod not found: web"));
        let e = classify_get_failure(
            &format!("Error from server (Forbidden): pods \"web\" is forbidden {secret}"),
            "web",
            None,
        );
        assert!(matches!(e, TransportError::Auth(_)));
        let e = classify_get_failure(
            "error: You must be logged in to the server (Unauthorized)",
            "web",
            None,
        );
        assert!(matches!(e, TransportError::Auth(_)));
        let e = classify_get_failure(
            "error: context \"stage\" does not exist",
            "web",
            Some("stage"),
        );
        assert!(matches!(&e, TransportError::Connect(m) if m.contains("no context `stage`")));
        let e = classify_get_failure(
            &format!("Unable to connect to the server: dial tcp {secret}: no route to host"),
            "web",
            None,
        );
        assert!(!e.to_string().contains("10.0.0.1") && !e.to_string().contains("abc"));
    }

    #[test]
    fn pty_script_exports_quoted_env() {
        let spec = PtySpec {
            term: "xterm-256color".into(),
            cols: 80,
            rows: 24,
            shell_cmd: "\"$HOME/.xxh/bin/zsh\" -l".into(),
            env: [("GREETING".to_string(), "it's $HOME".to_string())].into(),
        };
        assert_eq!(
            pty_script(&spec),
            "export TERM='xterm-256color'; export GREETING='it'\\''s $HOME'; \
             exec \"$HOME/.xxh/bin/zsh\" -l"
        );
    }

    /// The argument vectors `kubectl` receives (C-K8S7/C-K8S10), recorded by a
    /// stand-in script.
    #[test]
    fn kubectl_invocations() {
        let dir = std::env::temp_dir().join(format!(
            "xxh-fake-kubectl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        crate::container_backend::tests::write_script(
            &dir.join("kubectl"),
            &format!(
                "echo \"$*\" >> '{}'\n\
                 case \"$*\" in *' get pod '*) echo 'Running|side|app side';; \
                 *) echo out; echo err >&2; exit 3;; esac",
                log.display()
            ),
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            let mut t = KubectlTransport::with_binary(dir.join("kubectl"));
            let target = crate::KubeTarget {
                namespace: Some("pay".into()),
                context: Some("stage".into()),
                ..crate::KubeTarget::new("api-0")
            };
            t.connect(&target.into(), &AuthPolicy::default()).await?;
            let chosen = t.container().map(str::to_string);
            let out = t.exec("uname -m").await?;
            Ok::<_, TransportError>((chosen, out))
        });

        let calls = std::fs::read_to_string(&log).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let (chosen, out) = result.unwrap();
        assert_eq!(chosen.as_deref(), Some("side"));
        assert_eq!(out.exit_code, 3);
        assert_eq!(out.stdout_str(), "out");
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 2, "{calls}");
        assert!(
            lines[0].starts_with("--context stage -n pay get pod api-0 -o jsonpath="),
            "{}",
            lines[0]
        );
        assert!(lines[0].ends_with("--request-timeout=10s"), "{}", lines[0]);
        assert_eq!(
            lines[1],
            "--context stage -n pay exec -q api-0 -c side -- sh -c uname -m"
        );
    }

    /// kubectl's exit note is dropped, the command's own stderr is not (004 C-X1).
    #[test]
    fn exit_note_is_filtered() {
        let note = b"command terminated with exit code 3\n";
        let mut s = b"boom\n".to_vec();
        s.extend(note);
        assert_eq!(ExitNoteFilter::apply(&s, 3), b"boom\n");
        // Another code, exit 0, or output after it — not kubectl's note.
        assert_eq!(ExitNoteFilter::apply(note, 4), note);
        assert_eq!(ExitNoteFilter::apply(note, 0), note);
        let mut s = note.to_vec();
        s.extend(b"more\n");
        assert_eq!(ExitNoteFilter::apply(&s, 3), s);
        // Lookalikes pass through untouched.
        for text in [
            &b"command terminated early\n"[..],
            b"command terminated with exit code x\n",
            b"command",
            b"",
        ] {
            assert_eq!(ExitNoteFilter::apply(text, 1), text);
        }
        // Ordinary lines are emitted as they arrive, not held.
        let mut f = ExitNoteFilter::default();
        assert_eq!(f.feed(b"warn: a"), b"warn: a");
        assert_eq!(f.feed(b"b\ncomm"), b"b\n");
        assert_eq!(f.feed(b"and terminated with exit code 2\n"), b"");
        assert_eq!(f.finish(2), b"");
    }

    #[tokio::test]
    async fn rejects_other_families() {
        let mut t = KubectlTransport::new();
        for target in [
            ResolvedTarget::Ssh(crate::ResolvedSshTarget::new("h")),
            ResolvedTarget::Container(crate::ContainerTarget::new("c")),
        ] {
            let err = t.connect(&target, &AuthPolicy::default()).await;
            assert!(
                matches!(err, Err(TransportError::BackendUnavailable(_))),
                "got {err:?}"
            );
        }
    }
}
