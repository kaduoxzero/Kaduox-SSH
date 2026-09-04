# Changelog

All notable changes to Kaduox-SSH are documented here.

The project is still pre-1.0. Minor-version releases may add or adjust public APIs, but security boundaries and compatibility changes are called out explicitly.

## [0.9.0-rc.1] - Unreleased

### Added

- recursive directory download from the `kssh-tui` remote workspace with exact `YES` confirmation;
- recursive local-directory upload into the current TUI remote directory with root symlink rejection and exact `YES` confirmation;
- live per-file recursive transfer progress rendered from the existing bounded core progress event queue;
- direct remote path navigation from the TUI workspace with `p`.

### Safety and resource boundaries

- recursive TUI transfers reuse `SshClient::upload_recursive` / `download_recursive` and the canonical `TransferOptions` defaults rather than introducing a second scheduler;
- upload/download keep the existing bounded file concurrency, SFTP write window, request timeout, atomic per-file policy, and no-follow symlink scan behavior;
- recursive actions refuse the wrong selected/source type before starting and require an explicit confirmation token;
- remote progress paths continue through the TUI terminal escaping boundary before display;
- destructive remote mutations such as delete/rename remain intentionally out of the TUI until a separate confirmation and recovery model is designed.

### Validation status

`integration/v0.9.0-candidate` is wired into CI, Quality, and real OpenSSH push workflows. Candidate promotion to `develop` or `main` remains gated on those jobs actually acquiring runners and executing successfully.

## [0.7.0-rc.1] - Unreleased

### Added

- multi-host `kssh-tui` Session Dashboard backed by `ConnectionManager` and explicit `ConnectionLease` ownership;
- no-target TUI startup into an empty dashboard, with OpenSSH Host catalog selection and arbitrary target entry;
- persistent authenticated sessions that survive entering/leaving the existing remote SFTP/shell workspace;
- per-session close/disconnect plus visible connection-pool, in-use, lease, and capacity counters;
- bounded Dashboard Broadcast (`b`) that executes one operator-authored command across all currently open authenticated sessions without reconnecting;
- dedicated `kssh-fleet` binary for bounded-concurrency command execution across multiple SSH targets;
- fleet typed-command support for `--cwd`, repeatable `--env NAME=VALUE`, `--as-user`, `user@host`, OpenSSH aliases, ProxyJump/ProxyCommand, and the existing host-key/authentication policies;
- dedicated real-OpenSSH fleet integration fixture covering multi-target success, preflight no-side-effect behavior, runtime failure isolation, output truncation, and terminal-control escaping.

### Changed

- v0.7 TUI connection ownership is explicit: every open dashboard session owns a lease, preventing idle pruning until the session is closed;
- opening an already-present logical TUI target selects the existing session instead of silently creating another login;
- fleet command/config preflight resolves all targets before opening the first SSH connection, so malformed target syntax or unsupported OpenSSH configuration cannot produce a partially executed fleet operation;
- fleet scheduling keeps at most `--jobs` active SSH tasks and prints completed host results immediately rather than retaining every target's output until the end;
- fleet authentication now keeps one secret-bearing template and creates concrete authentication values only for targets entering the bounded in-flight window;
- CI, Quality, and OpenSSH workflows explicitly trigger on `integration/v0.7.0-candidate` pushes.

### Resource and security boundaries

- `kssh-fleet` defaults to 8 concurrent targets, hard-limits concurrency to 64, and limits one invocation to 1024 targets;
- fleet retained output defaults to 1 MiB stdout + 1 MiB stderr per in-flight target, caps each stream at 16 MiB, and rejects configurations where `jobs × output-limit × 2` exceeds 512 MiB;
- fleet output beyond the retention cap is still drained from SSH to avoid remote-process deadlock and is marked `truncated` locally;
- Dashboard Broadcast retains at most 256 KiB stdout + 256 KiB stderr per open session, for about 32 MiB at the manager's 64-connection default;
- aggregated fleet and broadcast output escape ESC, carriage-return, and other terminal-control characters by default;
- Dashboard Broadcast never interpolates remote filenames, paths, host labels, status text, or other server-controlled values into the operator-authored command;
- fleet preflight documentation now distinguishes configuration-time checks from transport-time ProxyCommand expansion safety, host-key verification, and authentication; all remain fail-closed without falsely claiming those transport checks happen before every independent host begins execution.

### Validation status

The v0.7 candidate has CI, Quality, and real OpenSSH workflows wired directly to candidate pushes, including the new fleet integration suite. GitHub Actions creates the runs and jobs, but the repository is still experiencing platform/runner-allocation failures before any step executes (`steps=[]` / no job logs). This is not a passing validation and is also not a compiler/test failure. Promotion to `develop` or `main` remains blocked until the jobs actually acquire runners and complete successfully.

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
