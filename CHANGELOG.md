# Changelog

All notable changes to Kaduox-SSH are documented here.

The project is still pre-1.0. Minor-version releases may add or adjust public APIs, but security boundaries and compatibility changes are called out explicitly.

## [0.6.0-rc.1] - Unreleased

### Added

- dedicated `kssh-tui` terminal UI binary without introducing a second SSH stack or an additional UI framework dependency;
- no-argument OpenSSH Host picker backed by a reusable core host catalog;
- full-screen SFTP remote browser with deterministic sorting, lstat-style metadata, safe terminal rendering, and parent-directory navigation;
- TUI interactive shell and sudo-backed shell actions on the existing authenticated transport;
- TUI single-file upload/download actions with conservative symlink/path handling;
- OpenSSH-style `user@host` targets, including bracketed IPv6 target syntax;
- side-effect-free `inspect` command and reusable redacted connection snapshots;
- authenticated `probe` command with the actual final-server host-key algorithm, SHA-256 fingerprint, and verification source;
- typed remote command specification with `exec --cwd` and repeatable `--env NAME=VALUE`;
- read-only SFTP `ls` and `stat` commands with stable core-owned metadata types;
- managed remote-forward lifecycle with explicit server-side `cancel-tcpip-forward` support;
- reusable connection-manager snapshots and explicit lease handling for long-lived frontends.

### Changed

- command execution streams stdout/stderr incrementally instead of requiring complete output buffering for the normal CLI path;
- recursive transfer planning/execution uses bounded memory/resource budgets and bounded progress queues;
- recursive privileged upload supports directory trees while keeping SFTP unprivileged;
- atomic local-to-remote sync performs fail-before-mutation preflight checks when an atomic overwrite cannot be guaranteed;
- OpenSSH route diagnostics now use the same effective precedence as the real transport (`ProxyJump` before `ProxyCommand` before direct);
- v0.6 candidate branches explicitly trigger CI, Quality, and real OpenSSH workflows on push so release work is not hidden behind PR-base filters.

### Security

- ProxyCommand token expansion is bounded and restricted to a portable shell-token grammar for untrusted expanded values;
- SSH connect, authentication, channel-open, channel-request, SFTP initialization, forwarding, and SOCKS handshakes have explicit bounds;
- server-initiated forwarded channels are rejected unless registered and are protected by a per-connection resource budget;
- remote forwarding unregisters the local route before sending cancellation so late forwarded channels fail closed;
- recursive transfer/sync path traversal and symlink boundaries were hardened;
- authentication secrets are excluded from `Debug` output;
- unsupported OpenSSH structural directives such as `Include` and `Match` continue to fail closed in connection resolution;
- the TUI escapes remote-controlled terminal text and refuses path-bearing remote directory-entry names;
- direct RSA private-key signing remains disabled while RSA authentication through an external SSH agent is allowed under the documented policy.

### Validation status

The v0.6 candidate has real GitHub Actions runs wired for CI, Quality, and OpenSSH integration. At the time this release candidate was prepared, jobs were created but failed before executing any step (`steps=[]`), indicating runner-allocation/platform failure rather than a test or compiler failure. Promotion to `develop`/`main` remains blocked until those jobs actually execute and pass.

## [0.3.0] - Previous development baseline

The repository entered the current development cycle at workspace version `0.3.0`. The v0.6 release candidate consolidates the SSH transport, transfer/sync hardening, forwarding lifecycle, diagnostics, connection-management, and TUI work developed since that baseline.
