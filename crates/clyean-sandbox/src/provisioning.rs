// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Provisioning of a populated root filesystem: base packages and a Java runtime, the
//! pinned PlantUML jar, the harness binary, and a verification that all three work.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clyean_plantuml::render::{CommandOutcome, CommandRunner};
use clyean_plantuml::{PlantUmlDistribution, PINNED_DISTRIBUTION};
use sha2::{Digest, Sha256};

use crate::container::HARNESS_CONTAINER_PATH;
use crate::fs::{SandboxArchive, SandboxFs};
use crate::podman::Podman;
use crate::rootfs::RootfsMarker;
use crate::roots::SandboxLocation;
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
    location: SandboxLocation,
}

impl RootfsRunner {
    pub fn new(podman: Podman, location: SandboxLocation) -> Self {
        Self { podman, location }
    }
}

impl CommandRunner for RootfsRunner {
    fn run(&self, argv: &[String]) -> std::io::Result<CommandOutcome> {
        let (label, value) = self.location.label();
        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--init".into(),
            "--label".into(),
            format!("{label}={value}"),
            "--rootfs".into(),
            self.location.root.clone(),
        ];
        args.extend(argv.iter().cloned());
        let output = self.podman.command(&args).output()?;
        Ok(CommandOutcome {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A Linux executable that Clyean runs inside the sandbox and publishes as a release asset
/// per architecture: the harness, installed into the root filesystem, and the bridge,
/// mounted into User Assistant containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxExecutable {
    /// The release asset name without its architecture tag.
    pub asset_stem: &'static str,
    /// The environment variable that names a host copy to use instead.
    pub override_env: &'static str,
    /// The name a copy beside the running `clyean` executable may have.
    pub sibling_name: &'static str,
    /// The cache subdirectory that holds downloaded copies.
    pub cache_subdir: &'static str,
}

pub const HARNESS: SandboxExecutable = SandboxExecutable {
    asset_stem: "clyean-harness-linux",
    override_env: "CLYEAN_HARNESS_BINARY",
    sibling_name: "clyean-harness",
    cache_subdir: "harness",
};

/// The bridge must be a static (musl) build so that it runs in any sandbox image.
pub const BRIDGE: SandboxExecutable = SandboxExecutable {
    asset_stem: "clyean-bridge-linux",
    override_env: "CLYEAN_BRIDGE_BINARY",
    sibling_name: "clyean-bridge",
    cache_subdir: "bridge",
};

impl SandboxExecutable {
    pub fn asset_name(&self, arch_tag: &str) -> String {
        format!("{}-{arch_tag}", self.asset_stem)
    }
}

/// Where a sandbox executable was found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExecutable {
    pub path: PathBuf,
    pub origin: ExecutableOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableOrigin {
    EnvironmentOverride,
    ProjectConfig,
    SiblingOfExecutable,
    ReleaseDownload,
}

pub const RELEASE_REPOSITORY: &str = "skiller3/clyean";

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

/// Resolves a sandbox executable for the architecture `arch_tag` in order: its environment
/// override, the project's configured path, a sibling of the running executable, then the
/// release asset of `clyean_version`, downloaded once into the cache and verified against
/// the release's `SHA256SUMS`.
pub async fn resolve_sandbox_executable(
    executable: &SandboxExecutable,
    project_override: Option<&Path>,
    clyean_version: &str,
    arch_tag: &str,
    cache_dir: &Path,
) -> Result<ResolvedExecutable> {
    if let Some(path) = std::env::var_os(executable.override_env) {
        return existing(PathBuf::from(path), ExecutableOrigin::EnvironmentOverride);
    }
    if let Some(path) = project_override {
        return existing(path.to_path_buf(), ExecutableOrigin::ProjectConfig);
    }
    if let Some(sibling) = sibling_of_executable(executable, arch_tag) {
        return Ok(ResolvedExecutable {
            path: sibling,
            origin: ExecutableOrigin::SiblingOfExecutable,
        });
    }
    let asset = executable.asset_name(arch_tag);
    let cached = cache_dir
        .join(executable.cache_subdir)
        .join(clyean_version)
        .join(&asset);
    if !cached.is_file() {
        download_release_asset(clyean_version, &asset, &cached)
            .await
            .map_err(|error| {
                SandboxError::ExecutableMissing(format!(
                    "{asset} for clyean {clyean_version} is not available ({error}); set {} to a Linux build of it, or place {} beside the clyean executable",
                    executable.override_env, executable.sibling_name
                ))
            })?;
    }
    Ok(ResolvedExecutable {
        path: cached,
        origin: ExecutableOrigin::ReleaseDownload,
    })
}

async fn download_release_asset(clyean_version: &str, asset: &str, target: &Path) -> Result<()> {
    let checksums_url = release_asset_url(clyean_version, "SHA256SUMS");
    let checksums = download_text(&checksums_url).await?;
    let expected =
        digest_from_checksums(&checksums, asset).ok_or_else(|| SandboxError::Download {
            url: checksums_url.clone(),
            reason: format!("SHA256SUMS has no entry for {asset}"),
        })?;
    download_verified(&release_asset_url(clyean_version, asset), target, &expected).await
}

/// Downloads an executable to `target`, keeping it only when its SHA-256 digest is
/// `expected`.
pub async fn download_verified(url: &str, target: &Path, expected: &str) -> Result<()> {
    download_to(url, target).await?;
    let actual = sha256_of(target)?;
    if actual != expected.to_ascii_lowercase() {
        let _ = std::fs::remove_file(target);
        return Err(SandboxError::Checksum {
            file: url.rsplit('/').next().unwrap_or(url).to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    set_executable(target)
}

fn existing(path: PathBuf, origin: ExecutableOrigin) -> Result<ResolvedExecutable> {
    if !path.is_file() {
        return Err(SandboxError::ExecutableMissing(format!(
            "{} does not exist ({origin:?})",
            path.display()
        )));
    }
    Ok(ResolvedExecutable { path, origin })
}

fn sibling_of_executable(executable: &SandboxExecutable, arch_tag: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [
        executable.asset_name(arch_tag),
        executable.sibling_name.to_string(),
    ]
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
    let reading = |e| SandboxError::io(format!("reading {}", path.display()), e);
    let mut file = std::fs::File::open(path).map_err(reading)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(reading)?;
    Ok(hex::encode(hasher.finalize()))
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
pub struct ProvisioningInputs<'a> {
    pub podman: &'a Podman,
    pub location: &'a SandboxLocation,
    pub fs: &'a dyn SandboxFs,
    pub user: &'a ContainerUser,
    pub image: &'a str,
    pub image_digest: &'a str,
    pub clyean_version: &'a str,
    pub plantuml_jar: &'a Path,
    pub harness_binary: &'a Path,
    pub project_dir: &'a Path,
}

/// Installs everything into a populated root filesystem and writes the marker.
pub fn provision(inputs: &ProvisioningInputs<'_>) -> Result<RootfsMarker> {
    let runner = RootfsRunner::new(inputs.podman.clone(), inputs.location.clone());
    run_script(
        &runner,
        &base_packages_script(inputs.user),
        "installing base packages",
    )?;
    inputs
        .fs
        .write(installation(inputs.plantuml_jar, inputs.harness_binary))?;
    let verification = run_script(
        &runner,
        &verification_script(&PINNED_DISTRIBUTION),
        "verifying the sandbox",
    )?;
    let provisioned_at = clyean_project::utc_now_rfc3339();
    let marker = RootfsMarker {
        sandbox_id: inputs.location.id.to_string(),
        image: inputs.image.to_string(),
        image_digest: inputs.image_digest.to_string(),
        provisioning_version: PROVISIONING_VERSION,
        clyean_version: inputs.clyean_version.to_string(),
        provisioned_at: provisioned_at.clone(),
        harness_version: harness_version(&verification),
        harness_sha256: sha256_of(inputs.harness_binary)?,
        project_dir: inputs.project_dir.to_string_lossy().into_owned(),
        last_used_at: provisioned_at,
    };
    marker.write(inputs.fs)?;
    Ok(marker)
}

/// Inputs of replacing the harness of a provisioned root filesystem.
pub struct HarnessReplacement<'a> {
    pub podman: &'a Podman,
    pub location: &'a SandboxLocation,
    pub fs: &'a dyn SandboxFs,
    pub harness_binary: &'a Path,
    pub harness_sha256: &'a str,
    pub marker: RootfsMarker,
}

/// Installs `harness_binary` as the harness of a provisioned root filesystem and records it
/// in the marker, leaving everything else in the root alone.  The new harness is written
/// beside the old one under a name of its own and moved over it only once it runs, so a
/// failed replacement keeps the old harness, containers starting meanwhile always find a
/// whole one, and running ones keep the harness they started with.
pub fn replace_harness(replacement: HarnessReplacement<'_>) -> Result<RootfsMarker> {
    let staging = staging_path(HARNESS_CONTAINER_PATH);
    let mut archive = SandboxArchive::new();
    archive.host_file(&staging, replacement.harness_binary, 0o755);
    replacement.fs.write(archive)?;
    let runner = RootfsRunner::new(replacement.podman.clone(), replacement.location.clone());
    let output = run_script(
        &runner,
        &replacement_script(&staging, HARNESS_CONTAINER_PATH),
        "replacing the harness",
    )?;
    let marker = RootfsMarker {
        harness_version: harness_version(&output),
        harness_sha256: replacement.harness_sha256.to_string(),
        ..replacement.marker
    };
    marker.write(replacement.fs)?;
    Ok(marker)
}

/// Script that runs the staged harness and, only when it runs, moves it over `target`.
fn replacement_script(staging: &str, target: &str) -> String {
    format!(
        "set -e\n\
         trap 'rm -f {staging}' EXIT\n\
         {staging} --version\n\
         mv -f {staging} {target}\n"
    )
}

/// A path beside `target` that no other staging, in this process or another, uses.
fn staging_path(target: &str) -> String {
    static STAGED: AtomicU64 = AtomicU64::new(0);
    let sequence = STAGED.fetch_add(1, Ordering::Relaxed);
    format!("{target}.{}-{sequence}.new", std::process::id())
}

/// The harness version from the output of `clyean --version` (`clyean/<version>`), which
/// ends the output of the scripts that run it.
fn harness_version(output: &str) -> String {
    output
        .lines()
        .last()
        .unwrap_or_default()
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
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

/// The pinned PlantUML jar with its stable link, and the harness binary.
fn installation(jar: &Path, harness: &Path) -> SandboxArchive {
    let distribution = &PINNED_DISTRIBUTION;
    let mut archive = SandboxArchive::new();
    archive.directory("/opt/plantuml", 0o755);
    archive.host_file(&distribution.sandbox_jar_path(), jar, 0o644);
    archive.symlink("/opt/plantuml/plantuml.jar", &distribution.jar_file_name());
    archive.host_file(HARNESS_CONTAINER_PATH, harness, 0o755);
    archive
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
        assert_eq!(HARNESS.asset_name("arm64"), "clyean-harness-linux-arm64");
        assert_eq!(BRIDGE.asset_name("x64"), "clyean-bridge-linux-x64");
        assert!(release_asset_url("0.2.0", "SHA256SUMS").ends_with("/v0.2.0/SHA256SUMS"));
        assert_eq!(
            release_asset_url("0.2.0", &HARNESS.asset_name("x64")),
            "https://github.com/skiller3/clyean/releases/download/v0.2.0/clyean-harness-linux-x64"
        );
    }

    #[cfg(unix)]
    #[test]
    fn installs_plantuml_and_harness_into_the_root() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let jar = source.path().join("plantuml.jar");
        let harness = source.path().join("harness");
        std::fs::write(&jar, b"jar").unwrap();
        std::fs::write(&harness, b"#!/bin/sh\necho clyean/1\n").unwrap();
        crate::fs::HostDirectoryFs::new(root.path())
            .write(installation(&jar, &harness))
            .unwrap();
        assert!(root
            .path()
            .join("opt/plantuml/plantuml-mit-1.2026.8.jar")
            .is_file());
        assert_eq!(
            std::fs::read_link(root.path().join("opt/plantuml/plantuml.jar")).unwrap(),
            Path::new("plantuml-mit-1.2026.8.jar")
        );
        let harness_mode = std::fs::metadata(root.path().join("usr/local/bin/clyean"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(harness_mode & 0o777, 0o755);
    }

    #[test]
    fn digests_are_streamed_from_the_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"abc").unwrap();
        assert_eq!(
            sha256_of(file.path()).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_harness_version_is_the_last_line_of_the_output() {
        assert_eq!(
            harness_version("openjdk 21\nPlantUML 1.2026.8\nclyean/18.2.7\n"),
            "18.2.7"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_staged_harness_replaces_the_old_one_only_once_it_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("clyean");
        let target = target.to_str().unwrap();
        let stage = |script: &str| {
            let staging = staging_path(target);
            std::fs::write(&staging, script).unwrap();
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
            staging
        };
        let replace = |staging: &str| {
            std::process::Command::new("sh")
                .args(["-c", &replacement_script(staging, target)])
                .output()
                .unwrap()
        };
        let old = "#!/bin/sh\necho clyean/1.0.0\n";
        std::fs::write(target, old).unwrap();

        let broken = stage("#!/bin/sh\nexit 3\n");
        assert!(!replace(&broken).status.success());
        assert!(
            !Path::new(&broken).exists(),
            "a failed replacement removes its staging file"
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), old);

        let working = stage("#!/bin/sh\necho clyean/2.0.0\n");
        let output = replace(&working);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            harness_version(&String::from_utf8_lossy(&output.stdout)),
            "2.0.0"
        );
        assert!(!Path::new(&working).exists());
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "#!/bin/sh\necho clyean/2.0.0\n"
        );
        assert_ne!(staging_path(target), staging_path(target));
    }

    #[tokio::test]
    async fn environment_override_must_point_at_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let result =
            resolve_sandbox_executable(&HARNESS, Some(&missing), "0.1.0", "x64", dir.path()).await;
        if std::env::var_os(HARNESS.override_env).is_none() {
            assert!(matches!(result, Err(SandboxError::ExecutableMissing(_))));
        }
    }
}
