// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Provisioning of a populated root filesystem: base packages and a Java runtime, the
//! pinned PlantUML jar, the harness binary, and a verification that all three work.

use std::path::{Path, PathBuf};

use clyean_plantuml::render::{CommandOutcome, CommandRunner};
use clyean_plantuml::{PlantUmlDistribution, PINNED_DISTRIBUTION};
use sha2::{Digest, Sha256};

use crate::container::HARNESS_CONTAINER_PATH;
use crate::podman::Podman;
use crate::rootfs::RootfsMarker;
use crate::user::ContainerUser;
use crate::{Result, SandboxError};

/// Bump when the provisioning steps change so existing sandboxes are re-provisioned.
pub const PROVISIONING_VERSION: u32 = 1;

/// Shell script that installs the base packages with whichever package manager the
/// image provides.  Java is required for PlantUML; git, curl, and certificates are
/// required by the agents; procps gives the agents `ps`.
pub fn base_packages_script(user: &ContainerUser) -> String {
    format!(
        "set -e\n\
         if command -v apt-get >/dev/null 2>&1; then\n\
         \x20 export DEBIAN_FRONTEND=noninteractive\n\
         \x20 apt-get update -q\n\
         \x20 apt-get install -y -q --no-install-recommends ca-certificates curl git openssh-client procps python3 default-jre-headless\n\
         \x20 rm -rf /var/lib/apt/lists/*\n\
         elif command -v apk >/dev/null 2>&1; then\n\
         \x20 apk add --no-cache ca-certificates curl git openssh-client procps python3 openjdk21-jre-headless libstdc++ libgcc\n\
         elif command -v dnf >/dev/null 2>&1; then\n\
         \x20 dnf install -y ca-certificates curl git openssh-clients procps-ng python3 java-21-openjdk-headless\n\
         \x20 dnf clean all\n\
         else\n\
         \x20 echo 'no supported package manager (apt-get, apk, dnf) found in the sandbox image' >&2\n\
         \x20 exit 1\n\
         fi\n\
         mkdir -p {home}/workspace /opt/plantuml /run/clyean /run/herdr\n",
        home = user.home()
    )
}

/// Script that proves the sandbox can render diagrams and run the harness.
pub fn verification_script(distribution: &PlantUmlDistribution) -> String {
    format!(
        "set -e\n\
         java -version 2>&1 | head -1\n\
         java -Djava.awt.headless=true -jar {jar} -version | head -1\n\
         {harness} --version\n",
        jar = distribution.sandbox_jar_path(),
        harness = HARNESS_CONTAINER_PATH,
    )
}

/// Runs commands inside a populated root filesystem with no project mounts.
#[derive(Debug, Clone)]
pub struct RootfsRunner {
    podman: Podman,
    root: PathBuf,
}

impl RootfsRunner {
    pub fn new(podman: Podman, root: impl Into<PathBuf>) -> Self {
        Self {
            podman,
            root: root.into(),
        }
    }
}

impl CommandRunner for RootfsRunner {
    fn run(&self, argv: &[String]) -> std::io::Result<CommandOutcome> {
        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--init".into(),
            "--rootfs".into(),
        ];
        args.push(self.root.to_string_lossy().into_owned());
        args.extend(argv.iter().cloned());
        let output = self.podman.command(&args).output()?;
        Ok(CommandOutcome {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Where a harness binary was found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessBinary {
    pub path: PathBuf,
    pub origin: HarnessOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessOrigin {
    EnvironmentOverride,
    ProjectConfig,
    SiblingOfExecutable,
    ReleaseDownload,
}

pub const HARNESS_BINARY_ENV: &str = "CLYEAN_HARNESS_BINARY";
pub const RELEASE_REPOSITORY: &str = "skiller3/clyean";

pub fn harness_asset_name(arch_tag: &str) -> String {
    format!("clyean-harness-linux-{arch_tag}")
}

pub fn harness_release_url(clyean_version: &str, arch_tag: &str) -> String {
    release_asset_url(clyean_version, &harness_asset_name(arch_tag))
}

pub fn release_asset_url(clyean_version: &str, asset: &str) -> String {
    format!("https://github.com/{RELEASE_REPOSITORY}/releases/download/v{clyean_version}/{asset}")
}

/// Finds the SHA-256 digest recorded for `asset` in a `SHA256SUMS` file (`<hex>  <name>`).
pub fn digest_from_checksums(checksums: &str, asset: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && digest.len() == 64).then(|| digest.to_ascii_lowercase())
    })
}

/// Resolves the harness binary in order: environment override, project configuration,
/// a sibling of the running executable, then the cached release download.
pub async fn resolve_harness_binary(
    project_override: Option<&Path>,
    clyean_version: &str,
    arch_tag: &str,
    cache_dir: &Path,
) -> Result<HarnessBinary> {
    if let Some(path) = std::env::var_os(HARNESS_BINARY_ENV) {
        return existing_binary(PathBuf::from(path), HarnessOrigin::EnvironmentOverride);
    }
    if let Some(path) = project_override {
        return existing_binary(path.to_path_buf(), HarnessOrigin::ProjectConfig);
    }
    if let Some(sibling) = sibling_harness(arch_tag) {
        return Ok(HarnessBinary {
            path: sibling,
            origin: HarnessOrigin::SiblingOfExecutable,
        });
    }
    let cached = cache_dir
        .join("harness")
        .join(clyean_version)
        .join(harness_asset_name(arch_tag));
    if !cached.is_file() {
        let asset = harness_asset_name(arch_tag);
        let checksums_url = release_asset_url(clyean_version, "SHA256SUMS");
        let checksums = download_text(&checksums_url).await?;
        let expected =
            digest_from_checksums(&checksums, &asset).ok_or_else(|| SandboxError::Download {
                url: checksums_url.clone(),
                reason: format!("SHA256SUMS has no entry for {asset}"),
            })?;
        let url = harness_release_url(clyean_version, arch_tag);
        download_to(&url, &cached).await?;
        let actual = sha256_of(&cached)?;
        if actual != expected {
            let _ = std::fs::remove_file(&cached);
            return Err(SandboxError::Checksum {
                file: asset,
                expected,
                actual,
            });
        }
        set_executable(&cached)?;
    }
    Ok(HarnessBinary {
        path: cached,
        origin: HarnessOrigin::ReleaseDownload,
    })
}

fn existing_binary(path: PathBuf, origin: HarnessOrigin) -> Result<HarnessBinary> {
    if !path.is_file() {
        return Err(SandboxError::HarnessMissing(format!(
            "{} does not exist ({origin:?})",
            path.display()
        )));
    }
    Ok(HarnessBinary { path, origin })
}

fn sibling_harness(arch_tag: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [harness_asset_name(arch_tag), "clyean-harness".to_string()]
        .into_iter()
        .map(|name| exe_dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Downloads the pinned PlantUML jar into the cache (verifying its checksum) unless it is
/// already there, and returns its path.
pub async fn ensure_plantuml_jar(cache_dir: &Path) -> Result<PathBuf> {
    let distribution = &PINNED_DISTRIBUTION;
    let target = cache_dir
        .join("plantuml")
        .join(distribution.jar_file_name());
    if target.is_file() && sha256_of(&target)? == distribution.sha256 {
        return Ok(target);
    }
    download_to(&distribution.download_url(), &target).await?;
    let actual = sha256_of(&target)?;
    if actual != distribution.sha256 {
        let _ = std::fs::remove_file(&target);
        return Err(SandboxError::Checksum {
            file: distribution.jar_file_name(),
            expected: distribution.sha256.to_string(),
            actual,
        });
    }
    Ok(target)
}

async fn download_text(url: &str) -> Result<String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| SandboxError::Download {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
    if !response.status().is_success() {
        return Err(SandboxError::Download {
            url: url.to_string(),
            reason: format!("HTTP {}", response.status()),
        });
    }
    response.text().await.map_err(|e| SandboxError::Download {
        url: url.to_string(),
        reason: e.to_string(),
    })
}

async fn download_to(url: &str, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SandboxError::io(format!("creating {}", parent.display()), e))?;
    }
    let response = reqwest::get(url)
        .await
        .map_err(|e| SandboxError::Download {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
    if !response.status().is_success() {
        return Err(SandboxError::Download {
            url: url.to_string(),
            reason: format!("HTTP {}", response.status()),
        });
    }
    let bytes = response.bytes().await.map_err(|e| SandboxError::Download {
        url: url.to_string(),
        reason: e.to_string(),
    })?;
    let partial = target.with_extension("partial");
    std::fs::write(&partial, &bytes)
        .map_err(|e| SandboxError::io(format!("writing {}", partial.display()), e))?;
    std::fs::rename(&partial, target)
        .map_err(|e| SandboxError::io(format!("moving {} into place", partial.display()), e))
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| SandboxError::io(format!("reading {}", path.display()), e))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| SandboxError::io(format!("marking {} executable", path.display()), e))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Inputs of one provisioning run.
#[derive(Debug, Clone)]
pub struct ProvisioningInputs<'a> {
    pub podman: &'a Podman,
    pub root: &'a Path,
    pub user: &'a ContainerUser,
    pub image: &'a str,
    pub image_digest: &'a str,
    pub clyean_version: &'a str,
    pub plantuml_jar: &'a Path,
    pub harness_binary: &'a Path,
}

/// Installs everything into a populated root filesystem and writes the marker.
pub fn provision(inputs: &ProvisioningInputs<'_>) -> Result<RootfsMarker> {
    let runner = RootfsRunner::new(inputs.podman.clone(), inputs.root);
    run_script(
        &runner,
        &base_packages_script(inputs.user),
        "installing base packages",
    )?;
    install_plantuml(inputs.root, inputs.plantuml_jar)?;
    install_harness(inputs.root, inputs.harness_binary)?;
    let verification = run_script(
        &runner,
        &verification_script(&PINNED_DISTRIBUTION),
        "verifying the sandbox",
    )?;
    let harness_version = verification
        .lines()
        .last()
        .unwrap_or_default()
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let marker = RootfsMarker {
        image: inputs.image.to_string(),
        image_digest: inputs.image_digest.to_string(),
        provisioning_version: PROVISIONING_VERSION,
        clyean_version: inputs.clyean_version.to_string(),
        provisioned_at: clyean_project::utc_now_rfc3339(),
        harness_version,
    };
    marker.write(inputs.root)?;
    Ok(marker)
}

fn run_script(runner: &RootfsRunner, script: &str, context: &str) -> Result<String> {
    let outcome = runner
        .run(&["sh".to_string(), "-c".to_string(), script.to_string()])
        .map_err(|e| SandboxError::io(context, e))?;
    if !outcome.succeeded() {
        return Err(SandboxError::NotProvisioned(format!(
            "{context} failed (exit {:?}): {}",
            outcome.exit_code,
            outcome.stderr.trim()
        )));
    }
    Ok(outcome.stdout)
}

fn install_plantuml(root: &Path, jar: &Path) -> Result<()> {
    let distribution = &PINNED_DISTRIBUTION;
    let install_dir = root.join("opt").join("plantuml");
    std::fs::create_dir_all(&install_dir)
        .map_err(|e| SandboxError::io(format!("creating {}", install_dir.display()), e))?;
    let target = install_dir.join(distribution.jar_file_name());
    std::fs::copy(jar, &target)
        .map_err(|e| SandboxError::io(format!("installing {}", target.display()), e))?;
    let link = install_dir.join("plantuml.jar");
    let _ = std::fs::remove_file(&link);
    symlink(&distribution.jar_file_name(), &link)
}

fn install_harness(root: &Path, binary: &Path) -> Result<()> {
    let target = root.join("usr").join("local").join("bin").join("clyean");
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SandboxError::io(format!("creating {}", parent.display()), e))?;
    }
    std::fs::copy(binary, &target).map_err(|e| {
        SandboxError::io(format!("installing the harness at {}", target.display()), e)
    })?;
    set_executable(&target)
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| SandboxError::io(format!("linking {}", link.display()), e))
}

#[cfg(not(unix))]
fn symlink(target: &str, link: &Path) -> Result<()> {
    let source = link.with_file_name(target);
    std::fs::copy(source, link)
        .map(|_| ())
        .map_err(|e| SandboxError::io(format!("copying {}", link.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_install_java_and_verify_every_component() {
        let script = base_packages_script(&ContainerUser::from_host_user_name("skyei"));
        assert!(script.contains("default-jre-headless"));
        assert!(
            script.contains("mkdir -p /home/skyei/workspace /opt/plantuml /run/clyean /run/herdr")
        );
        let verify = verification_script(&PINNED_DISTRIBUTION);
        assert!(verify.contains("/opt/plantuml/plantuml-mit-1.2026.8.jar -version"));
        assert!(verify.contains("/usr/local/bin/clyean --version"));
    }

    #[test]
    fn checksum_files_are_parsed_per_asset() {
        let checksums = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF  clyean-harness-linux-x64\nfeedface00000000000000000000000000000000000000000000000000000000 *clyean-linux-x64\n";
        assert_eq!(
            digest_from_checksums(checksums, "clyean-harness-linux-x64").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            digest_from_checksums(checksums, "clyean-linux-x64").as_deref(),
            Some("feedface00000000000000000000000000000000000000000000000000000000")
        );
        assert!(digest_from_checksums(checksums, "missing").is_none());
    }

    #[test]
    fn release_url_and_asset_names_follow_the_contract() {
        assert_eq!(harness_asset_name("arm64"), "clyean-harness-linux-arm64");
        assert!(release_asset_url("0.2.0", "SHA256SUMS").ends_with("/v0.2.0/SHA256SUMS"));
        assert_eq!(
            harness_release_url("0.2.0", "x64"),
            "https://github.com/skiller3/clyean/releases/download/v0.2.0/clyean-harness-linux-x64"
        );
    }

    #[test]
    fn installs_plantuml_and_harness_into_the_root() {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let jar = source.path().join("plantuml.jar");
        let harness = source.path().join("harness");
        std::fs::write(&jar, b"jar").unwrap();
        std::fs::write(&harness, b"#!/bin/sh\necho clyean/1\n").unwrap();
        install_plantuml(root.path(), &jar).unwrap();
        install_harness(root.path(), &harness).unwrap();
        assert!(root
            .path()
            .join("opt/plantuml/plantuml-mit-1.2026.8.jar")
            .is_file());
        assert!(root.path().join("opt/plantuml/plantuml.jar").exists());
        assert!(root.path().join("usr/local/bin/clyean").is_file());
    }

    #[tokio::test]
    async fn environment_override_must_point_at_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let result = resolve_harness_binary(Some(&missing), "0.1.0", "x64", dir.path()).await;
        if std::env::var_os(HARNESS_BINARY_ENV).is_none() {
            assert!(matches!(result, Err(SandboxError::HarnessMissing(_))));
        }
    }
}
