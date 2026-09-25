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
//! Each takes Cargo's verbosity flags after its name (`cargo build-bin -v`) and passes the
//! matching flag to every command it runs.  The bridge and the harness are Linux
//! executables for the architecture Podman runs containers on, so these commands expect a
//! Linux host (including WSL).

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const USAGE: &str =
    "usage: cargo <build-bin|buildlocal|build-cli|build-bridge> [-q|--quiet] [-v|--verbose]...";

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .context("xtask lives two levels below the workspace root")?
        .to_path_buf();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, flags) = args.split_first().context(USAGE)?;
    let build = LocalBuild {
        root,
        verbosity: Verbosity::parse(flags)?,
    };
    match command.as_str() {
        "build-bin" => build.all(),
        "build-cli" => build.cli(),
        "build-bridge" => build.bridge(sandbox_arch()?),
        _ => bail!(USAGE),
    }
}

/// How much the commands print, set with Cargo's own flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verbosity {
    Quiet,
    Normal,
    /// `-v` given this many times.
    Verbose(u8),
}

impl Verbosity {
    fn parse(flags: &[String]) -> Result<Self> {
        let mut quiet = false;
        let mut verbose: u8 = 0;
        for flag in flags {
            match flag.as_str() {
                "-q" | "--quiet" => quiet = true,
                "--verbose" => verbose += 1,
                short if short.len() > 1 && short[1..].chars().all(|c| c == 'v') => {
                    verbose += u8::try_from(short.len() - 1).unwrap_or(u8::MAX)
                }
                other => bail!("unexpected argument {other}\n{USAGE}"),
            }
        }
        match (quiet, verbose) {
            (true, 0) => Ok(Self::Quiet),
            (false, 0) => Ok(Self::Normal),
            (false, count) => Ok(Self::Verbose(count)),
            (true, _) => bail!("--quiet and --verbose cannot be used together"),
        }
    }

    fn cargo_flags(self) -> Vec<&'static str> {
        match self {
            Self::Quiet => vec!["--quiet"],
            Self::Normal => Vec::new(),
            Self::Verbose(count) => vec!["--verbose"; usize::from(count)],
        }
    }

    fn bun_install_flags(self) -> &'static [&'static str] {
        match self {
            Self::Quiet => &["--silent"],
            Self::Normal => &[],
            Self::Verbose(_) => &["--verbose"],
        }
    }

    /// `bun run` has no verbose flag.
    fn bun_run_flags(self) -> &'static [&'static str] {
        match self {
            Self::Quiet => &["--silent"],
            Self::Normal | Self::Verbose(_) => &[],
        }
    }

    fn curl_flags(self) -> &'static [&'static str] {
        match self {
            Self::Quiet => &["--silent", "--show-error"],
            Self::Normal => &[],
            Self::Verbose(_) => &["--verbose"],
        }
    }

    /// `tar` has no quiet flag; it lists nothing unless asked.
    fn tar_flags(self) -> &'static [&'static str] {
        match self {
            Self::Quiet | Self::Normal => &[],
            Self::Verbose(_) => &["-v"],
        }
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

struct LocalBuild {
    root: PathBuf,
    verbosity: Verbosity,
}

impl LocalBuild {
    fn all(&self) -> Result<()> {
        let arch = sandbox_arch()?;
        // Checked first so that a missing bun does not surface after minutes of Rust builds.
        if Command::new("bun").arg("--version").output().is_err() {
            bail!("building the harness needs bun on PATH (https://bun.sh); `cargo build-cli` builds clyean alone");
        }
        self.cli()?;
        self.bridge(arch)?;
        self.harness(arch)
    }

    fn cli(&self) -> Result<()> {
        self.run(
            cargo()
                .args(["install", "--path", "crates/clyean", "--root"])
                .arg(&self.root)
                .args(["--locked", "--force"])
                .args(self.verbosity.cargo_flags())
                .current_dir(&self.root),
        )
    }

    /// The bridge is mounted into containers of any image, so it is a static musl executable.
    fn bridge(&self, arch: Arch) -> Result<()> {
        self.run(
            cargo()
                .args(["install", "--path", "crates/clyean-bridge", "--root"])
                .arg(&self.root)
                .args(["--locked", "--force", "--target", arch.musl_target()])
                .args(self.verbosity.cargo_flags())
                .current_dir(&self.root),
        )
    }

    /// Builds the harness the way CI's `build-harness` job does, for one architecture, and
    /// installs it as `bin/clyean-harness-linux-<arch>`.
    fn harness(&self, arch: Arch) -> Result<()> {
        let omp = self.root.join("vendor/omp");
        self.run(
            Command::new("bun")
                .args(["install", "--frozen-lockfile"])
                .args(self.verbosity.bun_install_flags())
                .current_dir(&omp),
        )?;
        self.stage_native_addons(&omp, arch)?;
        let target = format!("linux-{}", arch.tag());
        self.run(
            Command::new("bun")
                .arg("run")
                .args(self.verbosity.bun_run_flags())
                .arg("ci:release:build-binaries")
                .env("RELEASE_TARGETS", &target)
                .current_dir(&omp),
        )?;
        let built = omp.join(format!("packages/coding-agent/binaries/omp-{target}"));
        let installed = self.root.join(format!("bin/clyean-harness-{target}"));
        std::fs::copy(&built, &installed)
            .with_context(|| format!("copying {} to {}", built.display(), installed.display()))?;
        if self.verbosity != Verbosity::Quiet {
            println!("Installed {}", installed.display());
        }
        Ok(())
    }

    /// Stages the prebuilt native addons of the vendored harness version, which the harness
    /// build embeds, from the npm package that upstream publishes for them.  Each version is
    /// downloaded once, into `target/xtask/`.
    fn stage_native_addons(&self, omp: &Path, arch: Arch) -> Result<()> {
        let version = vendored_harness_version(omp)?;
        let package = format!("pi-natives-linux-{}", arch.tag());
        let unpacked = self
            .root
            .join(format!("target/xtask/natives/{version}/{package}"));
        if !unpacked.is_dir() {
            let partial = unpacked.with_extension("partial");
            let _ = std::fs::remove_dir_all(&partial);
            std::fs::create_dir_all(&partial)?;
            let tarball = partial.join("package.tgz");
            self.run(
                Command::new("curl")
                    .args(["--fail", "--location", "--retry", "3"])
                    .args(self.verbosity.curl_flags())
                    .arg("-o")
                    .arg(&tarball)
                    .arg(native_addons_url(&package, &version)),
            )?;
            self.run(
                Command::new("tar")
                    .arg("-xzf")
                    .arg(&tarball)
                    .args(self.verbosity.tar_flags())
                    .arg("-C")
                    .arg(&partial),
            )?;
            std::fs::rename(&partial, &unpacked)?;
        }
        let prefix = format!("pi_natives.linux-{}", arch.tag());
        let staging = omp.join("packages/natives/native");
        for entry in std::fs::read_dir(unpacked.join("package"))? {
            let path = entry?.path();
            let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
            if name.starts_with(&prefix) && name.ends_with(".node") {
                std::fs::copy(&path, staging.join(name))
                    .with_context(|| format!("staging {name}"))?;
            }
        }
        Ok(())
    }

    /// Runs `command`, printed first like the install scripts print theirs.  When quiet, the
    /// command is not printed and its output is shown only if it fails, because not every
    /// command (the harness build script among them) has a quiet flag of its own.
    fn run(&self, command: &mut Command) -> Result<()> {
        let line = std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        if self.verbosity == Verbosity::Quiet {
            let output = command
                .output()
                .with_context(|| format!("starting {line}"))?;
            if !output.status.success() {
                let mut stderr = std::io::stderr();
                stderr.write_all(&output.stdout)?;
                stderr.write_all(&output.stderr)?;
                bail!("{line} failed ({})", output.status);
            }
            return Ok(());
        }
        eprintln!("+ {line}");
        let status = command
            .status()
            .with_context(|| format!("starting {line}"))?;
        if !status.success() {
            bail!("{line} failed ({status})");
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn verbosity(flags: &[&str]) -> Result<Verbosity> {
        Verbosity::parse(
            &flags
                .iter()
                .map(|flag| flag.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn verbosity_takes_cargos_flags() {
        assert_eq!(verbosity(&[]).unwrap(), Verbosity::Normal);
        assert_eq!(verbosity(&["-q"]).unwrap(), Verbosity::Quiet);
        assert_eq!(verbosity(&["--quiet"]).unwrap(), Verbosity::Quiet);
        assert_eq!(verbosity(&["-v"]).unwrap(), Verbosity::Verbose(1));
        assert_eq!(verbosity(&["-vv"]).unwrap(), Verbosity::Verbose(2));
        assert_eq!(
            verbosity(&["--verbose", "-v"]).unwrap(),
            Verbosity::Verbose(2)
        );
        assert!(verbosity(&["-q", "-v"]).is_err());
        assert!(verbosity(&["--release"]).is_err());
    }

    #[test]
    fn each_command_gets_its_own_flag_for_the_verbosity() {
        let quiet = Verbosity::Quiet;
        let normal = Verbosity::Normal;
        let very = Verbosity::Verbose(2);
        assert_eq!(quiet.cargo_flags(), ["--quiet"]);
        assert!(normal.cargo_flags().is_empty());
        assert_eq!(very.cargo_flags(), ["--verbose", "--verbose"]);
        assert_eq!(quiet.bun_install_flags(), ["--silent"]);
        assert_eq!(very.bun_install_flags(), ["--verbose"]);
        assert_eq!(quiet.bun_run_flags(), ["--silent"]);
        assert!(very.bun_run_flags().is_empty());
        assert_eq!(quiet.curl_flags(), ["--silent", "--show-error"]);
        assert!(normal.curl_flags().is_empty());
        assert_eq!(very.curl_flags(), ["--verbose"]);
        assert!(normal.tar_flags().is_empty());
        assert_eq!(very.tar_flags(), ["-v"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_quiet_command_reports_only_its_failure() {
        let build = LocalBuild {
            root: PathBuf::from("."),
            verbosity: Verbosity::Quiet,
        };
        assert!(build
            .run(Command::new("sh").args(["-c", "echo hidden"]))
            .is_ok());
        let error = build
            .run(Command::new("sh").args(["-c", "echo shown >&2; exit 3"]))
            .unwrap_err();
        assert!(
            error.to_string().contains("failed (exit status: 3)"),
            "{error}"
        );
    }

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
