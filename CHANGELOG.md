# Changelog

All notable changes to Kaduox-SSH are documented here.

The project is still pre-1.0. Minor-version releases may add or adjust public APIs, but security boundaries and compatibility changes are called out explicitly.

## [0.33.0-rc.3] - 2026-09-08

Windows desktop prerelease, on top of rc.1.

### Added (desktop)

- folders belong to a sidebar region (target hosts vs. dedicated jump servers) and can be deleted together with their member hosts after a two-step confirmation; deletions are refused while a member host is connected or referenced by a jump chain;
- saving a new host can auto-connect immediately (default on);
- clicking a connected host only switches to its session; a session tab bar lists all connections with switch/disconnect, and its ＋ adds a terminal to the current session;
- the two sidebar sections collapse and share a single scroll area;
- SFTP: create folders/files, rename, delete (empty directories only), properties dialog, current-directory name search, and in-place editing of UTF-8 text files up to 1 MiB;
- per-host command history persisted in `command-history.jsonl`: commands run in the app plus the remote bash/zsh history are captured, with copy/edit/re-run in the terminal-side panel;
- status bar and help page show the live app version;
- Windows overlay titlebar (no white native strip; draggable top bar).

### Fixed (desktop)

- SFTP panel row layout after adding the search toolbar.

### Storage

- Host library gains a backward-compatible `jump_folders` key; command history lives next to it in `command-history.jsonl`.

## [0.33.0-rc.1] - Unreleased

This candidate integrates the v0.16–v0.33 development lines and the daemon/host-library product line into `develop`.

### Added

- OS credential-store login-password persistence (`kssh credentials set/check/delete`) backed by Windows Credential Manager, macOS Keychain, or Linux Secret Service; when no explicit auth flag is given, a stored password is used before default identity discovery. Kaduox-SSH never writes passwords to its own files;
- `kssh completions <bash|zsh|fish|powershell|elvish>` shell completion generation;
- `kaduox-ssh-hosts` crate: atomic private TOML host database with aliases, recent-use ordering, strict codec validation, and transactional OpenSSH `Host` import; `kssh hosts` CRUD subcommands;
- named jump chains with ordered hop resolution (`kssh chains`) and interactive per-hop authentication;
- `kaduox-ssh-daemon` crate and `kssh-daemon` binary: per-user local IPC connection-reuse daemon with a bounded framed protocol, endpoint singleton/stale-socket handling, and platform peer identity verification; `kssh` exec/shell attempt daemon reuse before direct connection fallback;
- recent-aware fuzzy host picker in the CLI;
- Host Certificate and `@revoked` trust enforcement carried from v0.16–v0.17, including OpenSSH-hashed `known_hosts` markers;
- bounded OpenSSH `Include` resolution (wildcards, `${ENV}`, `%%`, `%l`/`%L`, nested scope restoration) and the bounded `Match all` / `Match originalhost` subset (v0.14, v0.19, v0.28–v0.31);
- explicit recursive transfer/sync symbolic-link policy (`skip` / fail-closed `reject`) across CLI, sync, and privileged recursive upload (v0.18);
- sync source preflight validating planned upload paths against the local tree before any remote mutation (v0.18);
- tracked, cancellable, retryable TUI transfer tasks and planned remote file mutations / recursive deletion (v0.10–v0.12);
- bounded TUI local file pickers for upload sources and recursive-download destinations (v0.32–v0.33);
- release pipeline hardening: per-target SPDX 2.3 SBOMs, GitHub artifact attestations, immutable action pins, pinned Rust 1.98.1 toolchain, published-artifact qualification, and native Windows/macOS signing for stable tags (v0.20–v0.25, v0.27);
- Windows NTFS ACL trust validation for OpenSSH user configuration reads (v0.26).

### Fixed

- repaired six corrupted `Cargo.lock` registry checksums so `--locked` builds verify against the crates.io index;
- removed `russh::client::Config.connection_timeout` literals (no such field in russh 0.63.1; connect timeouts are enforced through existing tokio wrappers);
- fully unwrapped recursive transfer/sync task results in the bounded JoinSet collectors;
- Windows mtime preservation now opens files with a write-capable handle (SetFileTime requires FILE_WRITE_ATTRIBUTES);
- Windows config/include test fixtures live under the trusted user profile so the fail-closed ACL trust check accepts them;
- inactive OpenSSH `Port` options are validated explicitly because russh-config silently ignores unparseable values.

## [0.15.0-rc.1] - Unreleased

### Security

- every OpenSSH user-config file read by the shared resolver — root `~/.ssh/config`, nested `Include`, and Host catalog discovery — now passes through one file-descriptor-bound trust path;
- Unix config files must be regular files owned by the process real uid or root and must not be writable by group/other (`mode & 0022 == 0`);
- the trusted uid comes from POSIX `getuid()` rather than `$HOME` ownership, so redirecting HOME cannot redefine the trusted configuration author;
- metadata verification and configuration reads use the same opened file descriptor, removing the path-based stat/read TOCTOU window;
- one physical config-file read is capped at 4 MiB before allocation, in addition to the existing cumulative Include input/output budgets;
- an insecure nested Include fails at the same trust boundary as the root configuration;
- Windows still requires a regular config file, but NTFS ACL parity is intentionally not claimed until a native Windows trust-policy implementation exists.

### Validation status

`integration/v0.15.0-candidate` is wired into CI, Quality, release-policy, and real OpenSSH push workflows. Promotion remains blocked until GitHub-hosted jobs actually acquire runners and execute checkout, formatting, compilation, tests, Clippy, audit, release-policy tests, and OpenSSH fixtures.

## [0.14.0-rc.1] - Unreleased

### Added

- bounded user-config OpenSSH `Include` expansion before host-scoped configuration resolution;
- support for absolute, `~/.ssh`-relative, current-user `~/...`, quoted/multiple Include paths, `*`/`?` wildcard expansion, lexical processing order, nested includes, and no-match continuation;
- OpenSSH-compatible restoration of the containing global/`Host` scope after every included file, preventing an included `Host` block from capturing later parent-file directives;
- implicit global user-config defaults are represented internally as `Host *` so the host-only downstream parser can preserve first-value-wins behavior;
- Host catalog discovery across the same bounded Include graph, allowing concrete aliases from included files to appear in the TUI host picker;
- `docs/OPENSSH_CONFIG.md` with the exact supported/rejected configuration and host-trust boundaries.

### Compatibility and safety boundaries

- wildcard Include patterns do not match leading-dot files unless the pattern begins with an explicit `.`, matching pathname glob behavior;
- Include nesting is capped at 16 levels, processed files at 256, total input/expanded configuration at 4 MiB, paths per directive at 64, one Include path at 16 KiB, and one wildcard component at 1024 bytes;
- active canonical paths are tracked to detect recursive Include cycles;
- filesystem read/metadata errors fail closed instead of being converted into an empty include;
- `Match` remains rejected for connection resolution; the read-only Host catalog may retain it only as an unsupported-structure marker;
- `%` token expansion, `${ENV}` expansion, `~other-user` expansion, and full bracket/collation glob expressions remain fail-closed until their OpenSSH semantics are reproduced exactly;
- OpenSSH-equivalent user-config owner/mode validation is not yet claimed across Unix ownership and Windows ACL models;
- OpenSSH host certificates and `known_hosts` `@cert-authority` remain a separate unsupported trust boundary.

### Validation status

`integration/v0.14.0-candidate` is wired into CI, Quality, release-policy, and real OpenSSH push workflows. Promotion remains blocked until GitHub-hosted jobs actually acquire runners and execute checkout, formatting, compilation, tests, Clippy, audit, release-policy tests, and OpenSSH fixtures.

## [0.13.0-rc.1] - Unreleased

### Added

- fail-closed release metadata validation covering workspace SemVer, local Cargo.lock package versions, exact tag identity, and the four-binary release suite;
- `scripts/release/release_tool.py` with coordinated workspace/Cargo.lock version synchronization and deterministic release packaging;
- Linux x86_64, macOS Intel, macOS Apple Silicon, and Windows x86_64 release archives containing `kssh`, `kssh-tui`, `kssh-fleet`, and `kssh-inventory`;
- post-build `--version` smoke tests for every released binary;
- per-suite JSON manifests with binary SHA-256/byte length plus a top-level `SHA256SUMS` covering all four archives;
- normalized archive ordering, timestamps, ownership metadata, and executable modes with repeatability tests;
- release-policy tests in the normal Quality workflow and a documented fail-closed release process in `docs/RELEASE.md`.

### Release security boundary

- release tags must exactly equal `v<workspace-version>` and the tagged commit must be reachable from `main`;
- SHA-256 integrity checks are not treated as publisher authentication;
- Windows Authenticode, macOS Developer ID signing/notarization, and final provenance/SBOM policy remain V1 release-security work;
- the repository workspace version intentionally remains `0.6.0-rc.1` until the lock-aware `set-version` path can be executed and reviewed in a runnable checkout.

### Validation status

The v0.13 candidate created CI, Quality, release-policy, and real OpenSSH jobs, but all jobs ended before any step executed (`steps=null`). That state is neither passing validation nor evidence of a source failure.

## [0.12.0-rc.1] - Unreleased

### Added

- frontend-neutral bounded `TransferTaskRegistry` with monotonic task IDs, `Queued` / `Running` / `Completed` / `Cancelled` / `Failed` states, latest progress, completion summaries, bounded error text, explicit cancellation state, and retry lineage;
- `TransferTaskManager` for tracked single-file and recursive upload/download without changing the existing direct `SshClient` transfer API signatures;
- core retry path that creates a new task record, records `retry_of`, and enables the canonical `TransferOptions.resume=true` staging/part-file policy;
- per-SSH-session transfer task history in `kssh-tui`, retained while the Dashboard session remains open;
- TUI `t` task panel for viewing recent tasks, clearing finished history, and resuming failed/cancelled work after exact `RESUME` confirmation.

### Resource and recovery boundaries

- task history defaults to 128 retained entries and has a hard maximum of 1024; capacity pressure evicts only the oldest terminal task and refuses to discard active work;
- retained source/destination labels and progress/error text are bounded before entering task history;
- tracked progress uses a bounded 64-entry internal queue and non-blocking downstream forwarding so task observation cannot backpressure SFTP or create unbounded memory growth;
- caller cancellation is propagated on a fixed 50ms interval into the registered task's canonical `TransferCancellation`, preserving the existing transfer cancellation model;
- TUI histories are isolated by `WorkspaceSession`, survive leaving/re-entering the remote workspace, and are released when that SSH session is closed;
- the TUI displays at most the 20 newest retained tasks and terminal-escapes task-controlled source, destination, progress, and error text;
- `clear` removes terminal task history only; queued/running tasks are preserved;
- only failed/cancelled tasks can be resumed, and resume creates a new task ID rather than overwriting the old record;
- resume is not rollback: already committed files remain committed and stable `.kaduox.part` / staging checkpoints may remain after another interruption;
- tracked local paths require valid UTF-8 so retry can reconstruct source/destination exactly; direct core transfer APIs retain their prior path behavior;
- no second transfer scheduler, SSH stack, SFTP stack, or UI framework dependency is introduced.

### Validation status

`integration/v0.12.0-candidate` is wired into CI, Quality, and real OpenSSH push workflows. Promotion remains blocked until GitHub-hosted jobs actually acquire runners and execute checkout, formatting, compile, tests, Clippy, audit, and real OpenSSH fixtures; runner-allocation failures are not treated as either passing validation or source failures.

## [0.11.0-rc.1] - Unreleased

### Added

- bounded two-phase `SshClient` recursive remote deletion with read-only planning, exact pre-write revalidation, post-order removal, and explicit plan summaries;
- `RemoteDeleteOptions`, `RemoteDeletePlan`, and `RemoteDeleteSummary`, with a 10,000-entry default planning budget and 100,000-entry hard limit;
- `kssh-tui` `X` action for planned recursive directory deletion after exact `DELETE TREE` confirmation;
- cancellable TUI single-file and recursive upload/download using the existing core `TransferCancellation` token;
- a dedicated bounded terminal cancellation listener for transfer-time `Esc` / `Ctrl-C` handling.

### Safety and resource boundaries

- recursive delete planning is read-only, requires a real directory root, never follows symlinks, and aborts on unsupported remote file types;
- before the first recursive-delete write, the complete tree is re-scanned and must exactly match the approved path/type plan;
- every recursive-delete entry is lstat-checked again immediately before removal and every directory is re-listed before `rmdir`;
- recursive tree deletion is explicitly non-transactional under SFTP v3, so the TUI does not offer user cancellation after mutation begins; protocol/race failures stop at the first error and may leave a partial tree;
- TUI transfer cancellation uses 100ms bounded `crossterm::event::poll` windows and explicitly stops/joins its listener, avoiding leaked blocking input tasks;
- transfer cancellation is cooperative rather than rollback: already completed files/directories and `.kaduox.part` staging artifacts may remain;
- recursive transfer progress continues to use the existing bounded core progress queue and no second scheduler is introduced;
- single-entry `x` / `Delete`, non-overwriting same-directory rename, and one-directory mkdir remain available as the safer narrow mutation primitives.

### Validation status

`integration/v0.11.0-candidate` is wired into CI, Quality, and real OpenSSH push workflows. Promotion remains blocked until GitHub-hosted jobs actually acquire runners and execute checkout, compile, tests, Clippy, audit, and OpenSSH fixtures; a `runner_id=0` / `steps=[]` failure is not treated as passing validation or as evidence of a source failure.

## [0.10.0-rc.1] - Unreleased

### Added

- fail-closed `SshClient` remote filesystem mutation APIs for one-directory creation, same-directory rename, and one-entry removal;
- `kssh-tui` `m` action for creating one directory in the current remote directory;
- `kssh-tui` `R` action for same-directory non-overwriting rename of the selected entry;
- `kssh-tui` `x` / `Delete` action for removing one regular file, one symbolic link itself, or one empty directory after exact `DELETE` confirmation.

### Safety boundaries

- remote mutation paths reject root/current-directory mutation, NUL/control characters, ambiguous `.` / `..` components, repeated separators, and trailing-directory paths;
- target occupancy uses lstat-style metadata, so dangling symbolic links remain occupied destinations instead of being mistaken for absent paths;
- rename is restricted to the same directory and rejects an existing destination; v0.10 exposes neither overwrite rename nor cross-directory move;
- symbolic-link deletion removes the link itself and never follows its target;
- directory removal performs an immediate directory listing and only permits an empty directory before issuing `rmdir`; recursive deletion is intentionally unavailable;
- TUI deletion requires the exact confirmation token `DELETE`, separate from the `YES` token used for recursive transfer confirmation;
- all mutations use the canonical bounded SFTP session path and frontends never receive a raw `SftpSession`.

### Validation status

`integration/v0.10.0-candidate` is wired into CI, Quality, and real OpenSSH push workflows. Promotion remains blocked until GitHub-hosted jobs actually acquire runners and execute repository steps; the previously observed `runner_id=0` / `steps=[]` condition is not treated as either passing validation or a source-code failure.

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
