// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Developer build commands, run through the aliases in `.cargo/config.toml`.  Each puts
//! executables into the repository's `bin/`, where a `clyean` built there finds the bridge
//! and the harness beside itself:
//!
//! - `build-bin` (alias `buildlocal`): `clyean`, the static bridge, and the harness.
//! - `build-cli`: `clyean` alone.
//! - `build-bridge`: the static bridge alone.
//!
//! The bridge and the harness are Linux executables for the architecture Podman runs
//! containers on, so these commands expect a Linux host (including WSL).

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .context("xtask lives two levels below the workspace root")?
        .to_path_buf();
    match std::env::args().nth(1).as_deref() {
        Some("build-bin") => build_bin(&root),
        Some("build-cli") => build_cli(&root),
        Some("build-bridge") => build_bridge(&root, sandbox_arch()?),
        _ => bail!("usage: cargo run -p xtask -- <build-bin|build-cli|build-bridge>"),
    }
}

/// The architecture of the Linux kernel that runs the containers, which `clyean` selects its
/// sandbox executables by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arch {
    X64,
    Arm64,
}

impl Arch {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "amd64" | "x86_64" | "x64" => Some(Self::X64),
            "arm64" | "aarch64" => Some(Self::Arm64),
            _ => None,
        }
    }

    /// The architecture tag of release asset names, as in `clyean-harness-linux-x64`.
    fn tag(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }

    fn musl_target(self) -> &'static str {
        match self {
            Self::X64 => "x86_64-unknown-linux-musl",
            Self::Arm64 => "aarch64-unknown-linux-musl",
        }
    }
}

/// Podman's architecture, as `clyean` asks for it, or this host's when Podman does not answer.
fn sandbox_arch() -> Result<Arch> {
    let podman = Command::new("podman")
        .args(["info", "--format", "{{.Host.Arch}}"])
        .output();
    let name = match podman {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => std::env::consts::ARCH.to_string(),
    };
    Arch::from_name(&name).with_context(|| format!("unsupported container architecture {name}"))
}

fn build_bin(root: &Path) -> Result<()> {
    let arch = sandbox_arch()?;
    // Checked first so that a missing bun does not surface after minutes of Rust builds.
    if Command::new("bun").arg("--version").output().is_err() {
        bail!("building the harness needs bun on PATH (https://bun.sh); `cargo build-cli` builds clyean alone");
    }
    build_cli(root)?;
    build_bridge(root, arch)?;
    build_harness(root, arch)
}

fn build_cli(root: &Path) -> Result<()> {
    run(cargo()
        .args(["install", "--path", "crates/clyean", "--root"])
        .arg(root)
        .args(["--locked", "--force"])
        .current_dir(root))
}

/// The bridge is mounted into containers of any image, so it is a static musl executable.
fn build_bridge(root: &Path, arch: Arch) -> Result<()> {
    run(cargo()
        .args(["install", "--path", "crates/clyean-bridge", "--root"])
        .arg(root)
        .args(["--locked", "--force", "--target", arch.musl_target()])
        .current_dir(root))
}

/// Builds the harness the way CI's `build-harness` job does, for one architecture, and
/// installs it as `bin/clyean-harness-linux-<arch>`.
fn build_harness(root: &Path, arch: Arch) -> Result<()> {
    let omp = root.join("vendor/omp");
    run(Command::new("bun")
        .args(["install", "--frozen-lockfile"])
        .current_dir(&omp))?;
    stage_native_addons(root, &omp, arch)?;
    let target = format!("linux-{}", arch.tag());
    run(Command::new("bun")
        .args(["run", "ci:release:build-binaries"])
        .env("RELEASE_TARGETS", &target)
        .current_dir(&omp))?;
    let built = omp.join(format!("packages/coding-agent/binaries/omp-{target}"));
    let installed = root.join(format!("bin/clyean-harness-{target}"));
    std::fs::copy(&built, &installed)
        .with_context(|| format!("copying {} to {}", built.display(), installed.display()))?;
    println!("Installed {}", installed.display());
    Ok(())
}

/// Stages the prebuilt native addons of the vendored harness version, which the harness
/// build embeds, from the npm package that upstream publishes for them.  Each version is
/// downloaded once, into `target/xtask/`.
fn stage_native_addons(root: &Path, omp: &Path, arch: Arch) -> Result<()> {
    let version = vendored_harness_version(omp)?;
    let package = format!("pi-natives-linux-{}", arch.tag());
    let unpacked = root.join(format!("target/xtask/natives/{version}/{package}"));
    if !unpacked.is_dir() {
        let partial = unpacked.with_extension("partial");
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial)?;
        let tarball = partial.join("package.tgz");
        run(Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&tarball)
            .arg(native_addons_url(&package, &version)))?;
        run(Command::new("tar")
            .arg("-xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&partial))?;
        std::fs::rename(&partial, &unpacked)?;
    }
    let prefix = format!("pi_natives.linux-{}", arch.tag());
    let staging = omp.join("packages/natives/native");
    for entry in std::fs::read_dir(unpacked.join("package"))? {
        let path = entry?.path();
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if name.starts_with(&prefix) && name.ends_with(".node") {
            std::fs::copy(&path, staging.join(name)).with_context(|| format!("staging {name}"))?;
        }
    }
    Ok(())
}

fn native_addons_url(package: &str, version: &str) -> String {
    format!("https://registry.npmjs.org/@oh-my-pi/{package}/-/{package}-{version}.tgz")
}

fn vendored_harness_version(omp: &Path) -> Result<String> {
    let manifest = omp.join("packages/coding-agent/package.json");
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    json["version"]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("{} has no version", manifest.display()))
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Runs `command`, printed first like the install scripts print theirs.
fn run(command: &mut Command) -> Result<()> {
    let line = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("+ {line}");
    let status = command
        .status()
        .with_context(|| format!("starting {line}"))?;
    if !status.success() {
        bail!("{line} failed ({status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architectures_map_podman_and_rust_names_to_asset_tags() {
        for (name, tag, target) in [
            ("amd64", "x64", "x86_64-unknown-linux-musl"),
            ("x86_64", "x64", "x86_64-unknown-linux-musl"),
            ("arm64", "arm64", "aarch64-unknown-linux-musl"),
            ("aarch64", "arm64", "aarch64-unknown-linux-musl"),
        ] {
            let arch = Arch::from_name(name).unwrap();
            assert_eq!((arch.tag(), arch.musl_target()), (tag, target), "{name}");
        }
        assert_eq!(Arch::from_name("riscv64"), None);
    }

    #[test]
    fn native_addons_come_from_the_npm_package_of_the_vendored_version() {
        assert_eq!(
            native_addons_url("pi-natives-linux-x64", "18.2.7"),
            "https://registry.npmjs.org/@oh-my-pi/pi-natives-linux-x64/-/pi-natives-linux-x64-18.2.7.tgz"
        );
        let omp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/omp");
        assert!(!vendored_harness_version(&omp).unwrap().is_empty());
    }
}
