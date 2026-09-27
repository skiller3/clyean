// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The user's relay allowlists and the host-only file that holds them.  An entry covers a
//! set of protocols, a host port range, a guest port range, and a set of host addresses;
//! entries belong to a project (by its canonical directory) or to a User Assistant session
//! (by its id, so they return when that conversation is resumed).  No container can reach
//! the file, which is the point: nothing in the sandbox can allowlist a relay.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// The file name of the relay file in Clyean's host configuration directory.
pub const RELAY_FILE_NAME: &str = "relays.json";

/// Session entries unused for this long are dropped whenever the file is written.
pub const SESSION_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The most host addresses one relay listens on.
pub const MAX_HOST_ADDRESSES: usize = 4;

/// The transport protocol of a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayProtocol {
    Tcp,
    Udp,
    Sctp,
}

impl RelayProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Sctp => "sctp",
        }
    }

    /// The name people read, for prompts.
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
            Self::Sctp => "SCTP",
        }
    }
}

impl FromStr for RelayProtocol {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            "sctp" => Ok(Self::Sctp),
            other => Err(format!("{other:?} is not tcp, udp, or sctp")),
        }
    }
}

/// A single port (`"3000"`), an inclusive range (`"3000-3010"`), or any port (`"any"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum PortRange {
    Any,
    Between(u16, u16),
}

impl PortRange {
    pub fn single(port: u16) -> Self {
        Self::Between(port, port)
    }

    pub fn contains(&self, port: u16) -> bool {
        match self {
            Self::Any => true,
            Self::Between(low, high) => (*low..=*high).contains(&port),
        }
    }
}

impl FromStr for PortRange {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("any") {
            return Ok(Self::Any);
        }
        let port = |part: &str| -> Result<u16, String> {
            match part.trim().parse::<u16>() {
                Ok(port) if port > 0 => Ok(port),
                _ => Err(format!("{part:?} is not a port from 1 to 65535")),
            }
        };
        let (low, high) = match text.split_once('-') {
            Some((low, high)) => (port(low)?, port(high)?),
            None => {
                let single = port(text)?;
                (single, single)
            }
        };
        if low > high {
            return Err(format!("the range {text:?} runs backwards"));
        }
        Ok(Self::Between(low, high))
    }
}

impl TryFrom<String> for PortRange {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<PortRange> for String {
    fn from(range: PortRange) -> Self {
        range.to_string()
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("any"),
            Self::Between(low, high) if low == high => write!(f, "{low}"),
            Self::Between(low, high) => write!(f, "{low}-{high}"),
        }
    }
}

/// One relay: from one host port, on one or more host addresses, to one guest port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySpec {
    pub protocol: RelayProtocol,
    pub host_port: u16,
    pub guest_port: u16,
    pub host_addresses: Vec<IpAddr>,
}

impl RelaySpec {
    /// The addresses a relay listens on when a request names none: IPv4 loopback, and
    /// IPv6 loopback when the host has it.
    pub fn default_addresses(ipv6_loopback: bool) -> Vec<IpAddr> {
        let mut addresses = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        if ipv6_loopback {
            addresses.push(IpAddr::V6(Ipv6Addr::LOCALHOST));
        }
        addresses
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.host_port == 0 || self.guest_port == 0 {
            return Err("host and guest ports run from 1 to 65535".into());
        }
        if self.host_addresses.is_empty() || self.host_addresses.len() > MAX_HOST_ADDRESSES {
            return Err(format!(
                "a relay listens on one to {MAX_HOST_ADDRESSES} host addresses"
            ));
        }
        for (index, address) in self.host_addresses.iter().enumerate() {
            if self.host_addresses[..index].contains(address) {
                return Err(format!("{address} is named twice"));
            }
            if address.is_multicast() {
                return Err(format!("{address} is a multicast address"));
            }
        }
        Ok(())
    }

    /// Whether every address is a loopback address, which only this computer reaches.
    pub fn is_loopback_only(&self) -> bool {
        self.host_addresses.iter().all(IpAddr::is_loopback)
    }

    /// The host addresses as people read them, `[::1]:3000` style.
    pub fn host_endpoints(&self) -> Vec<String> {
        self.host_addresses
            .iter()
            .map(|address| match address {
                IpAddr::V4(v4) => format!("{v4}:{}", self.host_port),
                IpAddr::V6(v6) => format!("[{v6}]:{}", self.host_port),
            })
            .collect()
    }
}

/// An allowlist entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllowEntry {
    pub protocols: Vec<RelayProtocol>,
    pub host_ports: PortRange,
    pub guest_ports: PortRange,
    pub host_addresses: Vec<IpAddr>,
}

impl AllowEntry {
    /// The entry a permanent answer records: exactly the request's protocol, ports, and
    /// addresses.
    pub fn exactly(spec: &RelaySpec) -> Self {
        Self {
            protocols: vec![spec.protocol],
            host_ports: PortRange::single(spec.host_port),
            guest_ports: PortRange::single(spec.guest_port),
            host_addresses: spec.host_addresses.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocols.is_empty() {
            return Err("an entry names at least one protocol".into());
        }
        if self.host_addresses.is_empty() {
            return Err("an entry names at least one host address".into());
        }
        Ok(())
    }

    /// Whether the entry covers the request.  Host and guest ranges are independent.  A
    /// request's address is covered when the entry lists it or the wildcard of its family
    /// (`0.0.0.0` or `::`); a request to listen on a wildcard is therefore covered only by
    /// that wildcard.
    pub fn covers(&self, spec: &RelaySpec) -> bool {
        self.protocols.contains(&spec.protocol)
            && self.host_ports.contains(spec.host_port)
            && self.guest_ports.contains(spec.guest_port)
            && spec.host_addresses.iter().all(|address| {
                let wildcard = match address {
                    IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                };
                self.host_addresses.contains(address) || self.host_addresses.contains(&wildcard)
            })
    }
}

impl fmt::Display for AllowEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let protocols: Vec<&str> = self.protocols.iter().map(|p| p.as_str()).collect();
        let addresses: Vec<String> = self.host_addresses.iter().map(IpAddr::to_string).collect();
        write!(
            f,
            "{} {} -> {} on {}",
            protocols.join(","),
            self.host_ports,
            self.guest_ports,
            addresses.join(", ")
        )
    }
}

/// Where an allowlist entry belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AllowScope {
    Project,
    Session,
}

/// The entries of one User Assistant session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAllowlist {
    pub project: String,
    pub last_used: String,
    #[serde(default)]
    pub allow: Vec<AllowEntry>,
}

/// The contents of the relay file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelaySettings {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub open_sign_in_pages: bool,
    #[serde(default)]
    pub projects: BTreeMap<String, Vec<AllowEntry>>,
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionAllowlist>,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            open_sign_in_pages: true,
            projects: BTreeMap::new(),
            sessions: BTreeMap::new(),
        }
    }
}

impl RelaySettings {
    /// The scope whose entries cover `spec`: the session's first, then the project's.
    pub fn covering_scope(
        &self,
        project: &str,
        session: Option<&str>,
        spec: &RelaySpec,
    ) -> Option<AllowScope> {
        let covers = |entries: &[AllowEntry]| entries.iter().any(|entry| entry.covers(spec));
        if covers(&self.session_entries(project, session)) {
            return Some(AllowScope::Session);
        }
        self.projects
            .get(project)
            .filter(|entries| covers(entries))
            .map(|_| AllowScope::Project)
    }

    /// The session's entries, when the session belongs to `project`.
    pub fn session_entries(&self, project: &str, session: Option<&str>) -> Vec<AllowEntry> {
        session
            .and_then(|id| self.sessions.get(id))
            .filter(|allowlist| allowlist.project == project)
            .map(|allowlist| allowlist.allow.clone())
            .unwrap_or_default()
    }

    pub fn project_entries(&self, project: &str) -> Vec<AllowEntry> {
        self.projects.get(project).cloned().unwrap_or_default()
    }

    /// Adds `entry` to a scope, unless an equal entry is already there.  A session entry
    /// without a session id cannot be recorded.
    pub fn add(
        &mut self,
        project: &str,
        session: Option<&str>,
        scope: AllowScope,
        entry: AllowEntry,
        now: &str,
    ) -> Result<(), String> {
        entry.validate()?;
        let entries = match scope {
            AllowScope::Project => self.projects.entry(project.to_string()).or_default(),
            AllowScope::Session => {
                let id = session.ok_or("the User Assistant has no session yet")?;
                let allowlist =
                    self.sessions
                        .entry(id.to_string())
                        .or_insert_with(|| SessionAllowlist {
                            project: project.to_string(),
                            last_used: now.to_string(),
                            allow: Vec::new(),
                        });
                allowlist.last_used = now.to_string();
                &mut allowlist.allow
            }
        };
        if !entries.contains(&entry) {
            entries.push(entry);
        }
        Ok(())
    }

    /// Removes the scope's entry at `index`, counting from zero in listing order.
    pub fn remove(
        &mut self,
        project: &str,
        session: Option<&str>,
        scope: AllowScope,
        index: usize,
    ) -> Result<AllowEntry, String> {
        let entries = match scope {
            AllowScope::Project => self.projects.get_mut(project),
            AllowScope::Session => session
                .and_then(|id| self.sessions.get_mut(id))
                .filter(|allowlist| allowlist.project == project)
                .map(|allowlist| &mut allowlist.allow),
        };
        match entries {
            Some(entries) if index < entries.len() => Ok(entries.remove(index)),
            _ => Err(format!("there is no entry number {}", index + 1)),
        }
    }

    /// Records that a session's entries were used just now.
    pub fn touch_session(&mut self, session: &str, now: &str) {
        if let Some(allowlist) = self.sessions.get_mut(session) {
            allowlist.last_used = now.to_string();
        }
    }

    /// Drops sessions unused for [`SESSION_RETENTION`], and projects left without entries.
    pub fn drop_stale(&mut self, now: OffsetDateTime) {
        let cutoff = now - SESSION_RETENTION;
        self.sessions.retain(|_, allowlist| {
            OffsetDateTime::parse(&allowlist.last_used, &Rfc3339)
                .map(|used| used >= cutoff)
                .unwrap_or(true)
        });
        self.projects.retain(|_, entries| !entries.is_empty());
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RelayFileError {
    #[error("{} does not parse ({message}); fix or remove it, and until then every relay is asked about", path.display())]
    Invalid { path: PathBuf, message: String },
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl RelayFileError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

/// The relay file: read at every evaluation, so hand edits apply at once, and written only
/// to record a permanent answer or an edit, under a lock, re-read first and replaced
/// atomically.  A file that does not parse is never overwritten.
#[derive(Debug, Clone)]
pub struct RelayFile {
    path: PathBuf,
}

impl RelayFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn in_config_dir(config_dir: &Path) -> Self {
        Self::new(config_dir.join(RELAY_FILE_NAME))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings, the defaults when there is no file, or an error for one that does
    /// not parse.
    pub fn load(&self) -> Result<RelaySettings, RelayFileError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RelaySettings::default())
            }
            Err(error) => {
                return Err(RelayFileError::io(
                    format!("reading {}", self.path.display()),
                    error,
                ))
            }
        };
        let settings: RelaySettings =
            serde_json::from_str(&text).map_err(|error| RelayFileError::Invalid {
                path: self.path.clone(),
                message: error.to_string(),
            })?;
        for entry in settings
            .projects
            .values()
            .flatten()
            .chain(settings.sessions.values().flat_map(|s| s.allow.iter()))
        {
            entry
                .validate()
                .map_err(|message| RelayFileError::Invalid {
                    path: self.path.clone(),
                    message,
                })?;
        }
        Ok(settings)
    }

    /// Applies `change` to the current settings and writes them back.
    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut RelaySettings) -> T,
    ) -> Result<T, RelayFileError> {
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(directory)
            .map_err(|e| RelayFileError::io(format!("creating {}", directory.display()), e))?;
        let lock_path = self.path.with_extension("json.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| RelayFileError::io(format!("opening {}", lock_path.display()), e))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _held = lock
            .write()
            .map_err(|e| RelayFileError::io(format!("locking {}", lock_path.display()), e))?;
        let mut settings = self.load()?;
        let outcome = change(&mut settings);
        settings.drop_stale(OffsetDateTime::now_utc());
        self.write(&settings)?;
        Ok(outcome)
    }

    fn write(&self, settings: &RelaySettings) -> Result<(), RelayFileError> {
        let mut text = serde_json::to_string_pretty(settings).expect("relay settings serialize");
        text.push('\n');
        let partial = self
            .path
            .with_extension(format!("json.{}.partial", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(&partial).and_then(|mut file| {
            file.write_all(text.as_bytes())
                .and_then(|_| file.sync_all())
        });
        if let Err(error) = written {
            let _ = std::fs::remove_file(&partial);
            return Err(RelayFileError::io(
                format!("writing {}", partial.display()),
                error,
            ));
        }
        std::fs::rename(&partial, &self.path)
            .map_err(|e| RelayFileError::io(format!("replacing {}", self.path.display()), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "/home/skyei/workspace/app";
    const NOW: &str = "2026-09-27T02:30:00Z";

    fn spec(protocol: RelayProtocol, host: u16, guest: u16, addresses: &[&str]) -> RelaySpec {
        RelaySpec {
            protocol,
            host_port: host,
            guest_port: guest,
            host_addresses: addresses.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    fn entry(
        protocols: &[RelayProtocol],
        host: &str,
        guest: &str,
        addresses: &[&str],
    ) -> AllowEntry {
        AllowEntry {
            protocols: protocols.to_vec(),
            host_ports: host.parse().unwrap(),
            guest_ports: guest.parse().unwrap(),
            host_addresses: addresses.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn port_ranges_parse_their_three_forms() {
        assert_eq!("3000".parse(), Ok(PortRange::single(3000)));
        assert_eq!("3000-3010".parse(), Ok(PortRange::Between(3000, 3010)));
        assert_eq!("ANY".parse(), Ok(PortRange::Any));
        for invalid in ["0", "65536", "3010-3000", "x", "", "1-"] {
            assert!(invalid.parse::<PortRange>().is_err(), "{invalid}");
        }
        assert_eq!(PortRange::Between(3000, 3010).to_string(), "3000-3010");
        assert_eq!(PortRange::single(80).to_string(), "80");
        assert!(PortRange::Between(3000, 3010).contains(3005));
        assert!(!PortRange::Between(3000, 3010).contains(2999));
    }

    #[test]
    fn host_and_guest_ranges_are_independent() {
        let allowed = entry(
            &[RelayProtocol::Tcp],
            "3000-3010",
            "3000-3010",
            &["127.0.0.1"],
        );
        assert!(allowed.covers(&spec(RelayProtocol::Tcp, 3001, 3007, &["127.0.0.1"])));
        assert!(!allowed.covers(&spec(RelayProtocol::Tcp, 3001, 4000, &["127.0.0.1"])));
        assert!(!allowed.covers(&spec(RelayProtocol::Udp, 3001, 3001, &["127.0.0.1"])));
    }

    #[test]
    fn wildcards_cover_their_family_and_a_wildcard_bind_needs_the_wildcard() {
        let ipv4_everywhere = entry(&[RelayProtocol::Tcp], "any", "any", &["0.0.0.0"]);
        assert!(ipv4_everywhere.covers(&spec(RelayProtocol::Tcp, 80, 80, &["192.168.1.20"])));
        assert!(ipv4_everywhere.covers(&spec(RelayProtocol::Tcp, 80, 80, &["0.0.0.0"])));
        assert!(!ipv4_everywhere.covers(&spec(RelayProtocol::Tcp, 80, 80, &["::1"])));
        let loopback = entry(&[RelayProtocol::Tcp], "any", "any", &["127.0.0.1", "::1"]);
        assert!(loopback.covers(&spec(RelayProtocol::Tcp, 80, 80, &["127.0.0.1", "::1"])));
        assert!(loopback.covers(&spec(RelayProtocol::Tcp, 80, 80, &["127.0.0.1"])));
        assert!(!loopback.covers(&spec(RelayProtocol::Tcp, 80, 80, &["0.0.0.0"])));
        let ipv6_everywhere = entry(&[RelayProtocol::Udp], "any", "any", &["::"]);
        assert!(ipv6_everywhere.covers(&spec(RelayProtocol::Udp, 53, 53, &["::1"])));
        assert!(!ipv6_everywhere.covers(&spec(RelayProtocol::Udp, 53, 53, &["127.0.0.1"])));
    }

    #[test]
    fn requests_are_validated() {
        assert!(spec(RelayProtocol::Tcp, 3000, 3000, &["127.0.0.1"])
            .validate()
            .is_ok());
        assert!(spec(RelayProtocol::Tcp, 0, 3000, &["127.0.0.1"])
            .validate()
            .is_err());
        assert!(spec(RelayProtocol::Tcp, 1, 1, &[]).validate().is_err());
        assert!(spec(RelayProtocol::Tcp, 1, 1, &["127.0.0.1", "127.0.0.1"])
            .validate()
            .is_err());
        assert!(spec(RelayProtocol::Tcp, 1, 1, &["224.0.0.1"])
            .validate()
            .is_err());
        let five = ["127.0.0.1", "::1", "0.0.0.0", "::", "10.0.0.1"];
        assert!(spec(RelayProtocol::Tcp, 1, 1, &five).validate().is_err());
        assert!(spec(RelayProtocol::Tcp, 1, 1, &["127.0.0.1", "::1"]).is_loopback_only());
        assert!(!spec(RelayProtocol::Tcp, 1, 1, &["0.0.0.0"]).is_loopback_only());
        assert_eq!(
            spec(RelayProtocol::Tcp, 3000, 1, &["127.0.0.1", "::1"]).host_endpoints(),
            ["127.0.0.1:3000", "[::1]:3000"]
        );
    }

    #[test]
    fn session_entries_come_first_and_only_for_their_project() {
        let mut settings = RelaySettings::default();
        let request = spec(RelayProtocol::Tcp, 54549, 54549, &["127.0.0.1", "::1"]);
        assert_eq!(settings.covering_scope(PROJECT, Some("s1"), &request), None);
        settings
            .add(
                PROJECT,
                None,
                AllowScope::Project,
                AllowEntry::exactly(&request),
                NOW,
            )
            .unwrap();
        assert_eq!(
            settings.covering_scope(PROJECT, Some("s1"), &request),
            Some(AllowScope::Project)
        );
        settings
            .add(
                PROJECT,
                Some("s1"),
                AllowScope::Session,
                AllowEntry::exactly(&request),
                NOW,
            )
            .unwrap();
        assert_eq!(
            settings.covering_scope(PROJECT, Some("s1"), &request),
            Some(AllowScope::Session)
        );
        assert_eq!(
            settings.covering_scope("/elsewhere", Some("s1"), &request),
            None,
            "a session's entries serve only its own project"
        );
        assert!(settings
            .add(
                PROJECT,
                None,
                AllowScope::Session,
                AllowEntry::exactly(&request),
                NOW
            )
            .is_err());
        settings
            .add(
                PROJECT,
                None,
                AllowScope::Project,
                AllowEntry::exactly(&request),
                NOW,
            )
            .unwrap();
        assert_eq!(
            settings.project_entries(PROJECT).len(),
            1,
            "duplicates are not added"
        );
        assert_eq!(
            settings
                .remove(PROJECT, None, AllowScope::Project, 0)
                .unwrap(),
            AllowEntry::exactly(&request)
        );
        assert!(settings
            .remove(PROJECT, None, AllowScope::Project, 0)
            .is_err());
    }

    #[test]
    fn stale_sessions_and_empty_projects_are_dropped() {
        let mut settings = RelaySettings::default();
        let request = spec(RelayProtocol::Tcp, 1, 1, &["127.0.0.1"]);
        settings
            .add(
                PROJECT,
                Some("old"),
                AllowScope::Session,
                AllowEntry::exactly(&request),
                "2026-07-01T00:00:00Z",
            )
            .unwrap();
        settings
            .add(
                PROJECT,
                Some("new"),
                AllowScope::Session,
                AllowEntry::exactly(&request),
                NOW,
            )
            .unwrap();
        settings.projects.insert("/empty".into(), Vec::new());
        settings.drop_stale(OffsetDateTime::parse(NOW, &Rfc3339).unwrap());
        assert!(!settings.sessions.contains_key("old"));
        assert!(settings.sessions.contains_key("new"));
        assert!(!settings.projects.contains_key("/empty"));
    }

    #[test]
    fn the_file_reads_as_defaults_when_missing_and_round_trips_hand_written_forms() {
        let dir = tempfile::tempdir().unwrap();
        let file = RelayFile::in_config_dir(&dir.path().join("clyean"));
        assert_eq!(file.load().unwrap(), RelaySettings::default());
        std::fs::create_dir_all(file.path().parent().unwrap()).unwrap();
        std::fs::write(
            file.path(),
            r#"{"enabled": false, "projects": {"/p": [{"protocols": ["tcp", "udp"], "hostPorts": "5000-5010", "guestPorts": "any", "hostAddresses": ["0.0.0.0"]}]}}"#,
        )
        .unwrap();
        let settings = file.load().unwrap();
        assert!(!settings.enabled);
        assert!(settings.open_sign_in_pages);
        assert!(settings
            .covering_scope(
                "/p",
                None,
                &spec(RelayProtocol::Udp, 5005, 9, &["10.1.2.3"])
            )
            .is_some());
        file.update(|settings| settings.enabled = true).unwrap();
        let text = std::fs::read_to_string(file.path()).unwrap();
        assert!(text.contains("\"hostPorts\": \"5000-5010\""), "{text}");
        assert!(file.load().unwrap().enabled);
    }

    #[test]
    fn a_file_that_does_not_parse_is_reported_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = RelayFile::in_config_dir(dir.path());
        std::fs::write(file.path(), "{ not json").unwrap();
        assert!(matches!(file.load(), Err(RelayFileError::Invalid { .. })));
        assert!(file.update(|settings| settings.enabled = false).is_err());
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "{ not json");
        std::fs::write(
            file.path(),
            r#"{"projects": {"/p": [{"protocols": [], "hostPorts": "1", "guestPorts": "1", "hostAddresses": ["127.0.0.1"]}]}}"#,
        )
        .unwrap();
        assert!(matches!(file.load(), Err(RelayFileError::Invalid { .. })));
    }
}
