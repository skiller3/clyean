// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! What Clyean learns about Podman before it starts any container: whether containers run
//! on this kernel or inside a Podman machine, whether both ends of Podman meet the floor
//! for this operating system, where Podman keeps its data, and, for a machine, its
//! provider and size.

use serde_json::Value;

use crate::podman::Podman;
use crate::roots::SandboxRoots;
use crate::{Result, SandboxError};

/// The CPUs and memory a Podman machine should have for Clyean.
pub const RECOMMENDED_CPUS: u64 = 4;
pub const RECOMMENDED_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Where Podman runs containers relative to the `clyean` process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    /// On the kernel `clyean` runs on (Linux, and Linux on WSL).
    SharedKernel,
    /// Inside a Podman machine reached through Podman's remote client.
    VirtualMachine,
}

/// The operating system `clyean` runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Linux,
    MacOs,
    Windows,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The oldest Podman release Clyean supports here.
    pub fn podman_floor(self) -> PodmanVersion {
        match self {
            Self::Linux => PodmanVersion::new(4, 9, 0),
            Self::MacOs | Self::Windows => PodmanVersion::new(5, 0, 0),
        }
    }

    /// The machine provider the installed Podman uses by default.
    pub fn default_provider(self) -> Option<&'static str> {
        match self {
            Self::Linux => None,
            Self::MacOs => Some("applehv"),
            Self::Windows => Some("wsl"),
        }
    }

    fn upgrade_advice(self) -> &'static str {
        match self {
            Self::Linux => "install Podman 4.9 or later from your distribution (Debian 12 packages 4.3.1; use Debian 13 or a Podman build from another source)",
            Self::MacOs => "run `brew upgrade podman`",
            Self::Windows => "run `winget upgrade RedHat.Podman`",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PodmanVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl PodmanVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses `5.7.0`, `4.9.3`, or `5.0.0-dev`, ignoring anything after the numbers.
    pub fn parse(text: &str) -> Option<Self> {
        let mut numbers = text.trim().split(['.', '-', '+']).map(str::parse::<u32>);
        let major = numbers.next()?.ok()?;
        let minor = numbers.next()?.ok()?;
        let patch = numbers.next().and_then(|n| n.ok()).unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }
}

impl std::fmt::Display for PodmanVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What `podman info`, `podman version`, and `podman machine info` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanEnvironment {
    pub topology: Topology,
    pub client_version: PodmanVersion,
    pub server_version: PodmanVersion,
    /// The architecture of the kernel that runs the containers.
    pub arch: String,
    pub rootless: bool,
    /// Where Podman keeps its images and containers, as a path on the Podman host.
    pub graph_root: String,
    pub cpus: u64,
    pub memory_bytes: u64,
    /// The Podman machine's provider, on native Windows and macOS.
    pub provider: Option<String>,
}

impl PodmanEnvironment {
    pub fn detect(podman: &Podman, host: HostOs) -> Result<Self> {
        let info = podman.output(["info", "--format", "json"])?;
        let version = podman.output(["version", "--format", "json"])?;
        let mut environment = Self::from_reports(&info, &version)?;
        if environment.topology == Topology::VirtualMachine && host != HostOs::Linux {
            environment.provider = podman
                .output(["machine", "info", "--format", "json"])
                .ok()
                .and_then(|text| provider_from_machine_info(&text));
        }
        Ok(environment)
    }

    /// Reads the environment from `podman info --format json` and `podman version
    /// --format json`.
    pub fn from_reports(info: &str, version: &str) -> Result<Self> {
        let info: Value = parse_report("info", info)?;
        let version: Value = parse_report("version", version)?;
        let host = &info["host"];
        let server_version = info
            .pointer("/version/Version")
            .and_then(Value::as_str)
            .and_then(PodmanVersion::parse)
            .ok_or_else(|| malformed("info", "version.Version"))?;
        let client_version = version
            .pointer("/Client/Version")
            .and_then(Value::as_str)
            .and_then(PodmanVersion::parse)
            .unwrap_or(server_version);
        let topology = if host["serviceIsRemote"].as_bool() == Some(true) {
            Topology::VirtualMachine
        } else {
            Topology::SharedKernel
        };
        Ok(Self {
            topology,
            client_version,
            server_version,
            arch: host["arch"]
                .as_str()
                .ok_or_else(|| malformed("info", "host.arch"))?
                .to_string(),
            rootless: host.pointer("/security/rootless").and_then(Value::as_bool) == Some(true),
            graph_root: info
                .pointer("/store/graphRoot")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("info", "store.graphRoot"))?
                .to_string(),
            cpus: host["cpus"].as_u64().unwrap_or(0),
            memory_bytes: host["memTotal"].as_u64().unwrap_or(0),
            provider: None,
        })
    }

    /// Stops when either end of Podman is older than the floor for `host`; otherwise
    /// returns the warnings to show once per launch.
    pub fn check(&self, host: HostOs) -> Result<Vec<String>> {
        let floor = host.podman_floor();
        if self.client_version < floor || self.server_version < floor {
            return Err(SandboxError::Invalid(format!(
                "Clyean needs Podman {}.{} or later here, but the Podman client is {} and the Podman service is {}; {}",
                floor.major,
                floor.minor,
                self.client_version,
                self.server_version,
                host.upgrade_advice()
            )));
        }
        let mut warnings = Vec::new();
        if self.topology == Topology::SharedKernel {
            return Ok(warnings);
        }
        match (self.provider.as_deref(), host.default_provider()) {
            (Some(provider), Some(default)) if provider == default => {}
            (Some("hyperv"), _) if host == HostOs::Windows => warnings.push(
                "This Podman machine uses the Hyper-V provider, which Clyean supports on a best-effort basis; the default WSL provider is fully supported.".to_string(),
            ),
            (Some(provider), Some(default)) => warnings.push(format!(
                "This Podman machine uses the {provider} provider, which Clyean does not support; recreate it with the default {default} provider."
            )),
            _ => {}
        }
        if self.cpus < RECOMMENDED_CPUS || self.memory_bytes < RECOMMENDED_MEMORY_BYTES {
            warnings.push(format!(
                "The Podman machine has {} CPUs and {:.1} GiB of memory; Clyean recommends at least {RECOMMENDED_CPUS} CPUs and 8 GiB.  {}",
                self.cpus,
                self.memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                resource_remedy(host, self.provider.as_deref())
            ));
        }
        Ok(warnings)
    }

    /// The architecture tag of Clyean's Linux release assets (`x64` or `arm64`).
    pub fn arch_tag(&self) -> Result<&'static str> {
        match self.arch.as_str() {
            "amd64" | "x86_64" => Ok("x64"),
            "arm64" | "aarch64" => Ok("arm64"),
            other => Err(SandboxError::Invalid(format!(
                "unsupported sandbox architecture {other}; Clyean supports amd64 and arm64"
            ))),
        }
    }

    /// The directories that hold sandbox root filesystems beside Podman's own data.
    pub fn sandbox_roots(&self) -> SandboxRoots {
        SandboxRoots::beside_graph_root(&self.graph_root)
    }
}

/// Translates a path on this host into the path the Podman host sees for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPathMapper {
    /// The same kernel, or a macOS machine whose shared directories keep their paths.
    Identity,
    /// A Windows machine, which sees drive `C:` at `/mnt/c`.
    WindowsDrives,
}

impl HostPathMapper {
    pub fn new(topology: Topology, host: HostOs) -> Self {
        match (topology, host) {
            (Topology::VirtualMachine, HostOs::Windows) => Self::WindowsDrives,
            _ => Self::Identity,
        }
    }

    pub fn map(self, path: &std::path::Path) -> String {
        let text = path.to_string_lossy();
        match self {
            Self::Identity => text.into_owned(),
            Self::WindowsDrives => windows_to_machine_path(&text),
        }
    }
}

/// `C:\Users\skye\app` becomes `/mnt/c/Users/skye/app`; anything without a drive letter
/// keeps its text, with separators turned into slashes.
fn windows_to_machine_path(path: &str) -> String {
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(drive), Some(':'), separator)
            if drive.is_ascii_alphabetic()
                && matches!(separator, None | Some('\\') | Some('/')) =>
        {
            let rest = path[2..].trim_start_matches(['\\', '/']).replace('\\', "/");
            let drive = drive.to_ascii_lowercase();
            if rest.is_empty() {
                format!("/mnt/{drive}")
            } else {
                format!("/mnt/{drive}/{rest}")
            }
        }
        _ => path.replace('\\', "/"),
    }
}

fn resource_remedy(host: HostOs, provider: Option<&str>) -> &'static str {
    match (host, provider) {
        (HostOs::Windows, Some("wsl")) => "Raise `processors` and `memory` in %UserProfile%\\.wslconfig, then run `wsl --shutdown` and start the machine again.",
        (HostOs::MacOs, _) => "A machine's size is fixed when it is created; recreating it with `podman machine init --cpus 4 --memory 8192` removes every sandbox stored in it.",
        _ => "Recreate the machine with at least 4 CPUs and 8 GiB of memory; that removes every sandbox stored in it.",
    }
}

fn provider_from_machine_info(text: &str) -> Option<String> {
    let info: Value = serde_json::from_str(text).ok()?;
    info.pointer("/Host/VMType")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn parse_report(command: &str, text: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|error| SandboxError::PodmanFailed {
        command: format!("{command} --format json"),
        stderr: format!("unexpected output: {error}"),
    })
}

fn malformed(command: &str, field: &str) -> SandboxError {
    SandboxError::PodmanFailed {
        command: format!("{command} --format json"),
        stderr: format!("the report has no {field}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_INFO: &str = r#"{"host": {"arch": "amd64", "cpus": 8, "memTotal": 16497885184, "serviceIsRemote": false, "security": {"rootless": true}}, "store": {"graphRoot": "/home/skyei/.local/share/containers/storage"}, "version": {"Version": "5.7.0"}}"#;
    const LOCAL_VERSION: &str = r#"{"Client": {"Version": "5.7.0"}}"#;

    fn machine(provider: &str, cpus: u64, memory_gib: u64) -> PodmanEnvironment {
        let info = format!(
            r#"{{"host": {{"arch": "arm64", "cpus": {cpus}, "memTotal": {}, "serviceIsRemote": true, "security": {{"rootless": true}}}}, "store": {{"graphRoot": "/var/home/core/.local/share/containers/storage"}}, "version": {{"Version": "5.6.1"}}}}"#,
            memory_gib * 1024 * 1024 * 1024
        );
        let mut environment = PodmanEnvironment::from_reports(
            &info,
            r#"{"Client": {"Version": "5.6.2"}, "Server": {"Version": "5.6.1"}}"#,
        )
        .unwrap();
        environment.provider = Some(provider.to_string());
        environment
    }

    #[test]
    fn a_local_podman_shares_the_kernel() {
        let environment = PodmanEnvironment::from_reports(LOCAL_INFO, LOCAL_VERSION).unwrap();
        assert_eq!(environment.topology, Topology::SharedKernel);
        assert_eq!(environment.client_version, PodmanVersion::new(5, 7, 0));
        assert_eq!(environment.arch_tag().unwrap(), "x64");
        assert!(environment.rootless);
        assert!(environment.check(HostOs::Linux).unwrap().is_empty());
    }

    #[test]
    fn a_remote_podman_is_a_machine_with_its_own_architecture() {
        let environment = machine("applehv", 4, 8);
        assert_eq!(environment.topology, Topology::VirtualMachine);
        assert_eq!(environment.server_version, PodmanVersion::new(5, 6, 1));
        assert_eq!(environment.client_version, PodmanVersion::new(5, 6, 2));
        assert_eq!(environment.arch_tag().unwrap(), "arm64");
        assert!(environment.check(HostOs::MacOs).unwrap().is_empty());
    }

    #[test]
    fn versions_below_the_floor_for_the_host_stop_the_launch() {
        let old = LOCAL_INFO.replace("5.7.0", "4.3.1");
        let environment =
            PodmanEnvironment::from_reports(&old, r#"{"Client": {"Version": "4.3.1"}}"#).unwrap();
        let message = environment.check(HostOs::Linux).unwrap_err().to_string();
        assert!(message.contains("Podman 4.9 or later"), "{message}");
        assert!(message.contains("Debian 12 packages 4.3.1"), "{message}");
        let linux_floor = LOCAL_INFO.replace("5.7.0", "4.9.3");
        let environment =
            PodmanEnvironment::from_reports(&linux_floor, r#"{"Client": {"Version": "4.9.3"}}"#)
                .unwrap();
        assert!(environment.check(HostOs::Linux).is_ok());
        assert!(environment.check(HostOs::MacOs).is_err());
        assert!(PodmanVersion::parse("5.0.0-dev").unwrap() >= HostOs::Windows.podman_floor());
    }

    #[test]
    fn providers_and_sizes_other_than_recommended_warn_without_stopping() {
        assert!(machine("wsl", 8, 16)
            .check(HostOs::Windows)
            .unwrap()
            .is_empty());
        let hyperv = machine("hyperv", 8, 16).check(HostOs::Windows).unwrap();
        assert!(hyperv[0].contains("best-effort"));
        let libkrun = machine("libkrun", 8, 16).check(HostOs::MacOs).unwrap();
        assert!(libkrun[0].contains("does not support"));
        let small = machine("wsl", 2, 4).check(HostOs::Windows).unwrap();
        assert!(small[0].contains(".wslconfig"), "{small:?}");
        let small = machine("applehv", 2, 16).check(HostOs::MacOs).unwrap();
        assert!(small[0].contains("fixed when it is created"), "{small:?}");
    }

    #[test]
    fn windows_paths_map_to_the_machines_drive_mounts() {
        let mapper = HostPathMapper::new(Topology::VirtualMachine, HostOs::Windows);
        assert_eq!(
            mapper.map(std::path::Path::new("C:\\Users\\skye\\workspace\\app")),
            "/mnt/c/Users/skye/workspace/app"
        );
        assert_eq!(mapper.map(std::path::Path::new("d:/data")), "/mnt/d/data");
        assert_eq!(mapper.map(std::path::Path::new("E:\\")), "/mnt/e");
        assert_eq!(
            HostPathMapper::new(Topology::VirtualMachine, HostOs::MacOs)
                .map(std::path::Path::new("/Users/skye/app")),
            "/Users/skye/app"
        );
        assert_eq!(
            HostPathMapper::new(Topology::SharedKernel, HostOs::Linux)
                .map(std::path::Path::new("/home/skye/app")),
            "/home/skye/app"
        );
    }

    #[test]
    fn the_provider_comes_from_machine_info() {
        let text =
            r#"{"Host": {"Arch": "arm64", "VMType": "applehv"}, "Version": {"Version": "5.6.2"}}"#;
        assert_eq!(provider_from_machine_info(text).as_deref(), Some("applehv"));
        assert_eq!(provider_from_machine_info("not json"), None);
    }
}
