//! Runtime detection of the remote host platform (T012, §FR-007).
//!
//! Parses the output of the bootstrap `detect` subcommand (`uname -s -m | caps`).
//! An unsupported platform must produce a clear error *before* any artefact is
//! written to the host (contracts/bootstrap-protocol.md C-B1).

use crate::ShellError;

/// Operating system family of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Darwin,
    FreeBsd,
    OpenBsd,
    NetBsd,
    Other,
}

/// CPU architecture of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Arm,
    Other,
}

/// C library flavour. Best-effort: `uname` does not report it, so it is refined
/// separately (analysis U1) and defaults to `Unknown`. Since first-party shells
/// ship as static musl builds, delivery does not depend on this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Libc {
    Glibc,
    Musl,
    Unknown,
}

/// Capabilities detected on the host that influence delivery (§VI).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCaps {
    pub has_tar: bool,
    pub has_gzip: bool,
    pub has_zstd: bool,
}

/// A resolved host platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
    pub libc: Libc,
    pub caps: HostCaps,
}

impl Platform {
    /// Parse the bootstrap `detect` line, e.g. `"Linux x86_64 | tar gzip zstd"`.
    pub fn parse_detect(line: &str) -> Result<Self, ShellError> {
        let (uname_part, caps_part) = match line.split_once('|') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (line.trim(), ""),
        };
        let mut it = uname_part.split_whitespace();
        let os_raw = it
            .next()
            .ok_or_else(|| ShellError::Unsupported(format!("empty detect output: {line:?}")))?;
        let arch_raw = it.next().ok_or_else(|| {
            ShellError::Unsupported(format!("no arch in detect output: {line:?}"))
        })?;

        let os = match os_raw {
            "Linux" => Os::Linux,
            "Darwin" => Os::Darwin,
            "FreeBSD" => Os::FreeBsd,
            "OpenBSD" => Os::OpenBsd,
            "NetBSD" => Os::NetBsd,
            _ => Os::Other,
        };
        let arch = match arch_raw {
            "x86_64" | "amd64" => Arch::X86_64,
            "aarch64" | "arm64" => Arch::Aarch64,
            s if s.starts_with("arm") || s.starts_with("armv") => Arch::Arm,
            _ => Arch::Other,
        };

        let caps = HostCaps {
            has_tar: caps_part.split_whitespace().any(|t| t == "tar"),
            has_gzip: caps_part.split_whitespace().any(|t| t == "gzip"),
            has_zstd: caps_part.split_whitespace().any(|t| t == "zstd"),
        };

        let p = Platform {
            os,
            arch,
            libc: Libc::Unknown,
            caps,
        };
        p.ensure_supported()?;
        Ok(p)
    }

    /// Reject platforms outside the supported matrix (§FR-007, Принцип II).
    fn ensure_supported(&self) -> Result<(), ShellError> {
        if matches!(self.os, Os::Other) || matches!(self.arch, Arch::Other) {
            return Err(ShellError::Unsupported(format!(
                "host platform {:?}/{:?} is not in the supported matrix",
                self.os, self.arch
            )));
        }
        Ok(())
    }

    /// Preferred delivery archive format given host capabilities (§VI): zstd if
    /// available, otherwise gzip (part of the minimal host contract).
    pub fn preferred_archive_fmt(&self) -> &'static str {
        if self.caps.has_zstd { "zst" } else { "gz" }
    }

    /// Canonical `<os>-<arch>` key used to select package payloads
    /// (`dist/<key>/` in shell packages, target tables in providers).
    pub fn target_key(&self) -> String {
        format!("{}-{}", self.os_str(), self.arch_str())
    }

    /// Lower-case OS name matching manifest target patterns (C-M5).
    pub fn os_str(&self) -> &'static str {
        match self.os {
            Os::Linux => "linux",
            Os::Darwin => "darwin",
            Os::FreeBsd => "freebsd",
            Os::OpenBsd => "openbsd",
            Os::NetBsd => "netbsd",
            Os::Other => "other",
        }
    }

    /// Canonical architecture name matching manifest target patterns.
    pub fn arch_str(&self) -> &'static str {
        match self.arch {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::Arm => "armv7l",
            Arch::Other => "other",
        }
    }

    /// Libc name for manifest target patterns (best-effort, U1).
    pub fn libc_str(&self) -> &'static str {
        match self.libc {
            Libc::Glibc => "glibc",
            Libc::Musl => "musl",
            Libc::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_glibc_host() {
        let p = Platform::parse_detect("Linux x86_64 | tar gzip zstd").unwrap();
        assert_eq!(p.os, Os::Linux);
        assert_eq!(p.arch, Arch::X86_64);
        assert!(p.caps.has_zstd);
        assert_eq!(p.preferred_archive_fmt(), "zst");
    }

    #[test]
    fn parses_alpine_busybox_host_without_zstd() {
        // Critical musl/BusyBox case: trimmed caps, only tar+gzip.
        let p = Platform::parse_detect("Linux aarch64 | tar gzip").unwrap();
        assert_eq!(p.arch, Arch::Aarch64);
        assert!(!p.caps.has_zstd);
        assert_eq!(p.preferred_archive_fmt(), "gz");
    }

    #[test]
    fn unsupported_platform_is_rejected() {
        let err = Platform::parse_detect("Plan9 sparc64 |");
        assert!(matches!(err, Err(ShellError::Unsupported(_))));
    }
}
