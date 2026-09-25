// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! One way for host code to read and write a sandbox root filesystem, whether it is an
//! ordinary directory on this host's kernel or lives inside a Podman machine.  Writes
//! travel as one tar archive built on the host with explicit modes and owner `0:0`, so a
//! host without Unix permissions or links (Windows) can still describe them.

use std::process::Stdio;

use crate::podman::Podman;
use crate::roots::SandboxLocation;
use crate::{Result, SandboxError};

/// Files, directories, and links to write into a root filesystem, by absolute path.
/// Files come from memory or from a host file, which is streamed rather than loaded.
#[derive(Debug, Default)]
pub struct SandboxArchive {
    entries: Vec<ArchiveEntry>,
}

#[derive(Debug)]
enum ArchiveEntry {
    Directory {
        path: String,
        mode: u32,
    },
    File {
        path: String,
        contents: Contents,
        mode: u32,
    },
    Symlink {
        path: String,
        target: String,
    },
}

#[derive(Debug)]
enum Contents {
    Bytes(Vec<u8>),
    HostFile(std::path::PathBuf),
}

impl SandboxArchive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn directory(&mut self, path: &str, mode: u32) {
        self.entries.push(ArchiveEntry::Directory {
            path: path.to_string(),
            mode,
        });
    }

    pub fn file(&mut self, path: &str, contents: impl Into<Vec<u8>>, mode: u32) {
        self.entries.push(ArchiveEntry::File {
            path: path.to_string(),
            contents: Contents::Bytes(contents.into()),
            mode,
        });
    }

    /// A file whose contents are read from `source` on this host when the archive is written.
    pub fn host_file(&mut self, path: &str, source: &std::path::Path, mode: u32) {
        self.entries.push(ArchiveEntry::File {
            path: path.to_string(),
            contents: Contents::HostFile(source.to_path_buf()),
            mode,
        });
    }

    pub fn symlink(&mut self, path: &str, target: &str) {
        self.entries.push(ArchiveEntry::Symlink {
            path: path.to_string(),
            target: target.to_string(),
        });
    }

    /// Writes the archive in tar format, owner `0:0`, to `writer`.
    pub fn write_tar(&self, writer: impl std::io::Write) -> std::io::Result<()> {
        let mut builder = tar::Builder::new(writer);
        for entry in &self.entries {
            match entry {
                ArchiveEntry::Directory { path, mode } => {
                    let mut header = header(tar::EntryType::Directory, *mode, 0);
                    builder.append_data(&mut header, relative(path), std::io::empty())?;
                }
                ArchiveEntry::File {
                    path,
                    contents: Contents::Bytes(bytes),
                    mode,
                } => {
                    let mut header = header(tar::EntryType::Regular, *mode, bytes.len() as u64);
                    builder.append_data(&mut header, relative(path), &bytes[..])?;
                }
                ArchiveEntry::File {
                    path,
                    contents: Contents::HostFile(source),
                    mode,
                } => {
                    let file = std::fs::File::open(source)?;
                    let size = file.metadata()?.len();
                    let mut header = header(tar::EntryType::Regular, *mode, size);
                    builder.append_data(&mut header, relative(path), file)?;
                }
                ArchiveEntry::Symlink { path, target } => {
                    let mut header = header(tar::EntryType::Symlink, 0o777, 0);
                    builder.append_link(&mut header, relative(path), target)?;
                }
            }
        }
        builder.into_inner()?.flush()
    }
}

fn header(kind: tar::EntryType, mode: u32, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default(),
    );
    header.set_size(size);
    header
}

fn relative(path: &str) -> &str {
    path.trim_start_matches('/')
}

/// Reads and writes one root filesystem.
pub trait SandboxFs: Send + Sync {
    /// The contents of each path, or `None` where no regular file exists.
    fn read_files(&self, paths: &[&str]) -> Result<Vec<Option<Vec<u8>>>>;

    fn write(&self, archive: SandboxArchive) -> Result<()>;

    fn remove(&self, paths: &[&str]) -> Result<()>;

    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.read_files(&[path])?.pop().flatten())
    }
}

/// A root filesystem that is a directory on this host's kernel.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub struct HostDirectoryFs {
    root: std::path::PathBuf,
}

#[cfg(unix)]
impl HostDirectoryFs {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn host_path(&self, path: &str) -> std::path::PathBuf {
        self.root.join(relative(path))
    }
}

#[cfg(unix)]
impl SandboxFs for HostDirectoryFs {
    fn read_files(&self, paths: &[&str]) -> Result<Vec<Option<Vec<u8>>>> {
        paths
            .iter()
            .map(|path| {
                let host_path = self.host_path(path);
                if !host_path.is_file() {
                    return Ok(None);
                }
                std::fs::read(&host_path)
                    .map(Some)
                    .map_err(|e| SandboxError::io(format!("reading {}", host_path.display()), e))
            })
            .collect()
    }

    fn write(&self, archive: SandboxArchive) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        for entry in archive.entries {
            let (path, result) = match &entry {
                ArchiveEntry::Directory { path, mode } => {
                    let target = self.host_path(path);
                    let result = std::fs::create_dir_all(&target).and_then(|()| {
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(*mode))
                    });
                    (path, result)
                }
                ArchiveEntry::File {
                    path,
                    contents,
                    mode,
                } => {
                    let target = self.host_path(path);
                    let result = prepare(&target)
                        .and_then(|()| match contents {
                            Contents::Bytes(bytes) => std::fs::write(&target, bytes),
                            Contents::HostFile(source) => std::fs::copy(source, &target).map(drop),
                        })
                        .and_then(|()| {
                            std::fs::set_permissions(
                                &target,
                                std::fs::Permissions::from_mode(*mode),
                            )
                        });
                    (path, result)
                }
                ArchiveEntry::Symlink { path, target: link } => {
                    let target = self.host_path(path);
                    let result =
                        prepare(&target).and_then(|()| std::os::unix::fs::symlink(link, &target));
                    (path, result)
                }
            };
            result.map_err(|e| {
                SandboxError::io(format!("writing {}", self.host_path(path).display()), e)
            })?;
        }
        Ok(())
    }

    fn remove(&self, paths: &[&str]) -> Result<()> {
        for path in paths {
            let host_path = self.host_path(path);
            let removed = if host_path.is_dir() && !host_path.is_symlink() {
                std::fs::remove_dir_all(&host_path)
            } else {
                std::fs::remove_file(&host_path)
            };
            match removed {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(SandboxError::io(
                        format!("removing {}", host_path.display()),
                        error,
                    ))
                }
            }
        }
        Ok(())
    }
}

/// Creates the parents of `path` and removes a file or link already there.
#[cfg(unix)]
fn prepare(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// A root filesystem on a Podman host that this process cannot reach directly.  Each call
/// runs one short-lived container on the root filesystem itself.
#[derive(Debug, Clone)]
pub struct PodmanHostFs {
    podman: Podman,
    root: String,
    label: (String, String),
}

/// Prints each named path as `F <size>` and its contents, or `M` when it is missing.
const READ_SCRIPT: &str = "for p in \"$@\"; do if [ -f \"$p\" ]; then printf 'F %s\\n' \"$(wc -c < \"$p\")\"; cat \"$p\"; else echo M; fi; done";

impl PodmanHostFs {
    pub fn new(podman: Podman, location: &SandboxLocation) -> Self {
        Self {
            podman,
            root: location.root.clone(),
            label: location.label(),
        }
    }

    fn command(&self, argv: &[&str]) -> std::process::Command {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--interactive".to_string(),
            "--network".to_string(),
            "none".to_string(),
            "--security-opt".to_string(),
            "label=disable".to_string(),
            "--label".to_string(),
            format!("{}={}", self.label.0, self.label.1),
            "--rootfs".to_string(),
            self.root.clone(),
        ];
        args.extend(argv.iter().map(|a| a.to_string()));
        let mut command = self.podman.command(args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command
    }

    fn run(&self, argv: &[&str], input: Option<SandboxArchive>) -> Result<Vec<u8>> {
        let mut command = self.command(argv);
        command.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = command.spawn().map_err(SandboxError::PodmanMissing)?;
        let writer = input.map(|archive| {
            let stdin = child.stdin.take().expect("stdin is piped");
            std::thread::spawn(move || archive.write_tar(stdin))
        });
        let output = child
            .wait_with_output()
            .map_err(SandboxError::PodmanMissing)?;
        if let Some(writer) = writer {
            if let Ok(Err(error)) = writer.join() {
                return Err(SandboxError::io("streaming files into the sandbox", error));
            }
        }
        if !output.status.success() {
            return Err(SandboxError::PodmanFailed {
                command: format!("run --rootfs {} {}", self.root, argv.join(" ")),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(output.stdout)
    }
}

impl SandboxFs for PodmanHostFs {
    fn read_files(&self, paths: &[&str]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut argv = vec!["sh", "-c", READ_SCRIPT, "sh"];
        argv.extend(paths);
        parse_read_output(&self.run(&argv, None)?, paths.len())
    }

    fn write(&self, archive: SandboxArchive) -> Result<()> {
        if archive.is_empty() {
            return Ok(());
        }
        self.run(&["tar", "-x", "-f", "-", "-C", "/"], Some(archive))?;
        Ok(())
    }

    fn remove(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut argv = vec!["rm", "-rf", "--"];
        argv.extend(paths);
        self.run(&argv, None)?;
        Ok(())
    }
}

/// Parses the output of [`READ_SCRIPT`] for `count` paths.
fn parse_read_output(mut output: &[u8], count: usize) -> Result<Vec<Option<Vec<u8>>>> {
    let malformed =
        || SandboxError::Invalid("unexpected output while reading sandbox files".into());
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let newline = output
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(malformed)?;
        let line = std::str::from_utf8(&output[..newline]).map_err(|_| malformed())?;
        output = &output[newline + 1..];
        match line.split_once(' ') {
            Some(("F", size)) => {
                let size: usize = size.trim().parse().map_err(|_| malformed())?;
                if output.len() < size {
                    return Err(malformed());
                }
                files.push(Some(output[..size].to_vec()));
                output = &output[size..];
            }
            _ if line == "M" => files.push(None),
            _ => return Err(malformed()),
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_carry_modes_owners_and_links() {
        let mut archive = SandboxArchive::new();
        assert!(archive.is_empty());
        archive.directory("/opt/plantuml", 0o755);
        archive.file("/opt/plantuml/plantuml-mit-1.jar", b"jar".to_vec(), 0o644);
        archive.symlink("/opt/plantuml/plantuml.jar", "plantuml-mit-1.jar");
        archive.file(
            "/home/skye/.omp/profiles/programmer/agent/clyean-credentials.json",
            b"{}".to_vec(),
            0o600,
        );
        let mut bytes = Vec::new();
        archive.write_tar(&mut bytes).unwrap();
        let mut reader = tar::Archive::new(&bytes[..]);
        let entries: Vec<(String, u32, u64, Option<String>)> = reader
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    entry.header().mode().unwrap(),
                    entry.header().uid().unwrap(),
                    entry
                        .link_name()
                        .unwrap()
                        .map(|l| l.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("opt/plantuml".to_string(), 0o755, 0, None),
                (
                    "opt/plantuml/plantuml-mit-1.jar".to_string(),
                    0o644,
                    0,
                    None
                ),
                (
                    "opt/plantuml/plantuml.jar".to_string(),
                    0o777,
                    0,
                    Some("plantuml-mit-1.jar".to_string())
                ),
                (
                    "home/skye/.omp/profiles/programmer/agent/clyean-credentials.json".to_string(),
                    0o600,
                    0,
                    None
                ),
            ]
        );
    }

    #[test]
    fn read_output_frames_present_and_missing_files() {
        let output = b"F 5\nhelloM\nF 0\nF 3\na\nb";
        let files = parse_read_output(output, 4).unwrap();
        assert_eq!(
            files,
            [
                Some(b"hello".to_vec()),
                None,
                Some(Vec::new()),
                Some(b"a\nb".to_vec())
            ]
        );
        assert!(parse_read_output(b"F 9\nshort", 1).is_err());
        assert!(parse_read_output(b"", 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_host_directory_reads_writes_and_removes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"#!/bin/sh\n").unwrap();
        let fs = HostDirectoryFs::new(dir.path());
        let mut archive = SandboxArchive::new();
        archive.host_file("/opt/tool/bin", source.path(), 0o755);
        archive.symlink("/opt/tool/link", "bin");
        archive.file("/secret.json", b"{}".to_vec(), 0o600);
        fs.write(archive).unwrap();
        let mut again = SandboxArchive::new();
        again.symlink("/opt/tool/link", "bin");
        again.file("/secret.json", b"{\"v\":2}".to_vec(), 0o600);
        fs.write(again).unwrap();
        let mode = |p: &str| {
            std::fs::metadata(dir.path().join(p))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("opt/tool/bin"), 0o755);
        assert_eq!(mode("secret.json"), 0o600);
        assert_eq!(
            std::fs::read_link(dir.path().join("opt/tool/link")).unwrap(),
            std::path::Path::new("bin")
        );
        assert_eq!(
            fs.read_files(&["/secret.json", "/missing", "/opt/tool"])
                .unwrap(),
            [Some(b"{\"v\":2}".to_vec()), None, None]
        );
        fs.remove(&["/opt", "/secret.json", "/never"]).unwrap();
        assert_eq!(fs.read_file("/secret.json").unwrap(), None);
        assert!(!dir.path().join("opt").exists());
    }
}
