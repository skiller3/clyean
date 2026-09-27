# Relay allowlist

A relay carries traffic from a port on your computer into the sandbox, and it opens only when you allow it in a prompt or when an entry of your allowlist covers it.  This page is the reference for the relay file that holds your allowlists, its schema, and how requests are matched against it.  [Relay ports and run servers](../how-to/relay-ports-and-run-servers.md) shows how to use relays.

## Location

`relays.json` lives in Clyean's host configuration directory, never in the project:

| Host | Directory |
| --- | --- |
| Linux, Linux on WSL, macOS | `$XDG_CONFIG_HOME/clyean` when `XDG_CONFIG_HOME` is set, otherwise `~/.config/clyean` |
| Native Windows | `%APPDATA%\clyean` |

The file does not exist until Clyean first writes it, and a missing file means the defaults below.  No container can read or write it: when your workspace contains the configuration directory (for example when the workspace is your home directory), every container gets an empty in-memory filesystem over that path.  Allowlists belong to you and this computer rather than to the project, because agents can edit every file of the project.

## Schema

```json
{
  "enabled": true,
  "openSignInPages": true,
  "projects": {
    "/home/me/workspace/app": [
      {"protocols": ["tcp"], "hostPorts": "3000", "guestPorts": "3000",
       "hostAddresses": ["127.0.0.1", "::1"]},
      {"protocols": ["tcp", "udp"], "hostPorts": "5000-5010", "guestPorts": "any",
       "hostAddresses": ["0.0.0.0"]}
    ]
  },
  "sessions": {
    "01J8Z3KX0W4Q2V5N7R9T1Y3B5D": {
      "project": "/home/me/workspace/app",
      "lastUsed": "2026-09-27T02:30:00Z",
      "allow": [
        {"protocols": ["tcp"], "hostPorts": "54549", "guestPorts": "54549",
         "hostAddresses": ["127.0.0.1", "::1"]}
      ]
    }
  }
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | `false` turns relaying off, and every request answers `relay_disabled`. |
| `openSignInPages` | `true` | `false` stops Clyean from opening sign-in pages in your browser; sign-in relays still open. |
| `projects` | `{}` | Entries allowed permanently for a project, keyed by the project's canonical directory.  A moved project starts with an empty list. |
| `sessions` | `{}` | Entries allowed permanently for one User Assistant session, keyed by its session id, with the project the session belongs to and when one of its entries last allowed a relay.  They return when you continue or resume that session. |

Each entry has exactly these fields:

| Field | Meaning |
| --- | --- |
| `protocols` | A non-empty list of `tcp`, `udp`, and `sctp`. |
| `hostPorts` | The ports on your computer: one port (`"3000"`), an inclusive range (`"3000-3010"`), or `"any"`. |
| `guestPorts` | The ports in the sandbox, in the same forms. |
| `hostAddresses` | One to four addresses.  `0.0.0.0` stands for every IPv4 address and `::` for every IPv6 address. |

## Matching

A request is covered by an entry when all of these hold:

- its protocol is in `protocols`;
- its host port is in `hostPorts` and its guest port is in `guestPorts`, independently, so an entry for host ports `3000-3010` and guest ports `3000-3010` also covers host port 3001 to guest port 3007;
- each of its host addresses is listed in `hostAddresses` or falls under the listed wildcard of its family;
- a request to listen on a wildcard itself (`0.0.0.0` or `::`) is covered only by an entry that lists that wildcard.

A request that names no host addresses listens on `127.0.0.1` and, when this computer has IPv6 loopback, on `::1` as well, so an entry covers it only when it lists both loopback addresses or the wildcards.

Clyean consults the entries of the current session first, then those of the project.  A covered request opens without a prompt and reports the scope that allowed it (`session` or `project`); any other request is put to you in a prompt.

## Writing

You can edit the file by hand.  Clyean reads it at every request, so an edit applies at once.

Clyean itself writes the file only to record a permanent answer to a prompt, which adds an entry with exactly the request's protocol, ports, and addresses, and to apply an edit from `/clyean relays`.  It takes a lock (`relays.json.lock` beside the file), re-reads the file, and replaces it atomically.  Session entries not used for 30 days are dropped whenever it writes.

A file that does not parse counts as empty.  Every request is then put to you, prompts offer only "Allow once" and "Deny" and say why, and Clyean does not overwrite the file until you fix it.

## Editing from the User Assistant

`/clyean relays` lists the open relays and both allowlists, and offers to add an entry, remove one, or close a relay.  The one-line forms are:

```text
/clyean relays allow <project|session> <protocols> <host ports> <guest ports> <addresses>
/clyean relays revoke <project|session> <number>
```

For example, `/clyean relays allow project tcp,udp 5000-5010 any 0.0.0.0` adds the second project entry of the example above.  The numbers are the ones `/clyean relays` shows.  Clyean accepts allowlist edits only from the User Assistant's own extension, over its lease, never from an agent's tool or another process.
