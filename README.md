# Kaduox-SSH

**English** | [简体中文](README.zh-CN.md)

Kaduox-SSH is a Rust-first SSH client focused on long-term maintainability, low latency, bounded memory usage, reproducible builds, and a reusable core shared by CLI, TUI, and fleet frontends.

## Current capabilities

- SSH connection, command execution, and interactive PTY shell
- OpenSSH `~/.ssh/config` resolution for supported host-scoped directives, with fail-closed handling for unsupported structural directives
- ProxyJump chains and ProxyCommand transports
- password, keyboard-interactive, Ed25519/ECDSA private-key, OpenSSH-agent, and Pageant/Windows-agent authentication paths
- RSA authentication through an external SSH agent while local RSA private-key signing is disabled by security policy
- explicit early rejection of RSA-only server host keys while the affected RSA verification feature is disabled
- opt-in agent forwarding
- local (`-L`), remote (`-R`), and SOCKS5 dynamic (`-D`) forwarding
- fast remote OS user switching over an existing SSH transport through `sudo`
- terminal resize propagation
- strict / accept-new / explicit-insecure host-key verification
- bounded-memory SFTP upload/download
- recursive directory transfers with bounded file concurrency
- resumable `.kaduox.part` transfers
- atomic destination staging by default
- transfer progress events and cooperative cancellation
- configurable SFTP packet size, pipelined write concurrency, and request timeout
- sudo-backed privileged single-file and recursive directory upload using unprivileged SFTP staging
- SFTP-native local-to-remote directory synchronization with non-mutating planning
- explicit `--delete` policy for destructive sync operations
- reusable in-process authenticated connection manager with explicit long-lived leases
- `kssh-tui` multi-host Session Dashboard with persistent authenticated sessions
- TUI OpenSSH Host picker, remote SFTP workspace, interactive shell/sudo shell, and regular-file upload/download
- bounded Dashboard Broadcast across already-open authenticated sessions
- `kssh-fleet` bounded-concurrency typed command execution across multiple SSH targets
- per-target fleet failure isolation, capped output retention, and terminal-safe aggregation
- real OpenSSH protocol integration fixtures, including fleet execution coverage
- Linux/macOS/Windows CI, Rust 1.85 MSRV, clippy, audit, and real-OpenSSH workflow gates
- committed `Cargo.lock` with `--locked` builds
- on-demand real-OpenSSH performance benchmark harness
- tag-driven Linux/macOS/Windows release packaging

The authenticated SSH login user cannot be changed after SSH authentication. Kaduox-SSH opens additional channels on the existing transport and uses `sudo -iu <user>` for interactive privilege switching or `sudo -n -u <user> -- sh -lc ...` for commands.

Privileged upload does not pretend SFTP can change uid. Kaduox-SSH stages files or directory trees as the SSH login user, then performs an explicit sudo-backed install phase as the requested remote OS user and removes the staging data afterwards. Recursive privileged replacement uses a sibling work directory and a remove-then-rename step when the destination already exists, so replacement of an existing directory is deliberately not claimed to be atomic.

## Frontends

Kaduox-SSH currently ships three frontends over the same core security and transport implementation:

- `kssh`: single-target CLI for shell, exec, transfer, sync, forwarding, inspection, and diagnostics;
- `kssh-tui`: multi-host interactive Session Dashboard. Each open host owns an explicit `ConnectionLease`; entering/leaving its remote workspace does not reconnect. The dashboard can also broadcast one operator-authored command over all already-open sessions;
- `kssh-fleet`: non-interactive bounded-concurrency command execution over an explicit target list. It preflights target/config/typed-command input before opening the first SSH connection and isolates runtime failures per host.

The TUI and fleet frontends do not implement separate SSH stacks. They reuse `kaduox-ssh-core` for authentication, host-key policy, ProxyJump/ProxyCommand, channels, SFTP, privilege switching, and transport limits.

See `docs/TUI.md`, `docs/SESSION_WORKSPACE.md`, and `docs/FLEET_EXEC.md` for frontend-specific contracts and resource limits.

## RSA security policy

Kaduox-SSH intentionally disables Russh's optional local RSA signer while the dependency path is affected by `RUSTSEC-2023-0071` (the Marvin timing attack advisory). The affected `rsa` crate is therefore absent from the resolved application lockfile.

This policy distinguishes private-key signing from public-key compatibility:

- Ed25519 and ECDSA private-key files can be used directly.
- A local RSA private-key file fails closed with an explicit remediation message.
- RSA user authentication remains available through an external SSH agent because the private-key operation stays inside the agent rather than Kaduox-SSH.
- RSA-only server host keys are currently rejected during key-exchange negotiation. In Russh 0.63.1 the feature needed for RSA host-signature verification is coupled to the affected local RSA signer dependency, so Kaduox-SSH prefers a fail-closed compatibility tradeoff over advertising an algorithm it cannot safely verify. Servers should expose Ed25519 or ECDSA host keys.

The real OpenSSH integration suite continuously verifies direct RSA-key rejection, agent-backed RSA authentication, and early negotiation failure against an RSA-host-key-only `sshd` fixture. Do not re-enable the Russh `rsa` feature until the advisory has an acceptable upstream resolution and the full security test suite remains green.

## OpenSSH config compatibility

Kaduox-SSH resolves normal host-scoped OpenSSH settings such as `HostName`, `User`, `Port`, `IdentityFile`, `UserKnownHostsFile`, `ProxyCommand`, and `ProxyJump` from `~/.ssh/config`.

Configuration resolution is intentionally fail-closed when the current parser cannot preserve OpenSSH semantics safely:

- A missing `~/.ssh/config` is normal and falls back to direct/default connection settings.
- Read errors and parse errors in an existing config are returned to the caller; they are never silently converted into a direct connection.
- `Match` blocks are currently rejected because `russh-config 0.58.0` does not evaluate them correctly and can otherwise allow subordinate settings to bleed into an unrelated `Host` block.
- `Include` is currently rejected rather than pretending included files were resolved.
- The upstream parser exposes `StrictHostKeyChecking` only as a boolean. Values other than `no` therefore lose their exact OpenSSH policy. Use Kaduox-SSH's explicit `--host-key strict`, `--host-key accept-new`, or `--host-key insecure` when exact behavior matters.

The long-term target is lossless OpenSSH-compatible resolution, including `Include` and correctly evaluated `Match` semantics. Until that is implemented and protocol-tested, unsupported structural configuration fails explicitly instead of guessing.

## Architecture

```text
crates/
  kaduox-ssh-core/   # transport/auth/session/forward/SFTP/sync/privilege/manager core
  kaduox-ssh-cli/    # kssh + kssh-tui + kssh-fleet frontend package
```

The core is frontend-independent. TUI sessions use explicit connection leases over the same manager, while fleet operations use the same typed command and transport APIs with separate batch scheduling and output policies.

## Build

Rust 1.85+ is the declared MSRV.

```bash
cargo build --release --locked
```

The resulting frontend binaries are `kssh`, `kssh-tui`, and `kssh-fleet`.

## Examples

Authentication falls back to configured identities when agent authentication does not succeed.

```bash
# Interactive shell
kssh server.example.com --user deploy shell

# Root shell over the same SSH transport
kssh server.example.com --user deploy shell --as-user root

# Execute a command
kssh server.example.com --user deploy exec -- uname -a

# Execute as another remote OS user
kssh server.example.com --user deploy exec --as-user root -- id

# Direct private key (Ed25519/ECDSA) / password
kssh server.example.com --user deploy --identity ~/.ssh/id_ed25519 shell
kssh server.example.com --user deploy --password shell

# RSA keys remain usable through ssh-agent; direct RSA key files are rejected
ssh-add ~/.ssh/id_rsa
kssh server.example.com --user deploy shell

# ProxyJump and forwarding
kssh target.internal -J bastion.example -L 8080:127.0.0.1:80 tunnel
kssh target.internal -D 1080 tunnel

# Atomic single-file upload/download
kssh server.example.com upload ./app.tar.zst /tmp/app.tar.zst
kssh server.example.com download /var/log/app.log ./app.log

# Resume an interrupted transfer
kssh server.example.com upload ./large.img /srv/large.img --resume
kssh server.example.com download /srv/large.img ./large.img --resume

# Recursive directory transfers
kssh server.example.com upload ./dist /srv/www/dist -r --jobs 8
kssh server.example.com download /srv/logs ./logs -r --jobs 8

# Install a staged upload as root, without running SFTP as root
kssh server.example.com upload ./nginx.conf /etc/nginx/nginx.conf --as-user root --mode 0644

# Recursively install a staged tree as root; file and directory modes are explicit
kssh server.example.com upload ./dist /srv/www/app -r --as-user root --mode 0644 --dir-mode 0755 --jobs 8

# Inspect a sync plan without modifying the server
kssh server.example.com sync ./dist /srv/www/dist --dry-run

# Apply non-destructive synchronization
kssh server.example.com sync ./dist /srv/www/dist

# Mirror the local tree, explicitly allowing deletion of remote-only entries
kssh server.example.com sync ./dist /srv/www/dist --delete

# Faster comparisons when timestamps are unreliable
kssh server.example.com sync ./dist /srv/www/dist --size-only --jobs 8

# Open the multi-host TUI dashboard with one initial session
kssh-tui production

# Or start an empty dashboard and add hosts interactively
kssh-tui

# Run one typed command across a bounded fleet
kssh-fleet -H web-01 -H web-02 -H deploy@web-03 --jobs 3 -- uname -a

# Fleet command with typed cwd/environment and sudo-backed remote user
kssh-fleet -H app-01 -H app-02 --cwd /srv/app --env APP_ENV=prod --as-user root -- id
```

By default host keys use `accept-new`: unknown keys are written to the normal OpenSSH `known_hosts` file while changed keys are rejected. Use `--host-key strict` to require a pre-existing entry. `--host-key insecure` is intentionally explicit and should only be used for disposable/test environments.

## Transfer and sync design

Transfers use bounded buffers and leave large-file request pipelining to `russh-sftp`, while Kaduox-SSH controls higher-level policy such as stable resume files, atomic finalization, directory concurrency, progress, cancellation, and privileged staging. SFTP session limits are configurable locally and are still constrained by limits negotiated with the remote server.

Transfer tuning is fail-closed rather than unbounded: file concurrency is limited to 128, pipelined SFTP writes to 128, packet size to 4 KiB–4 MiB, and the estimated `file_concurrency × write_concurrency × packet_size` window must stay at or below 512 MiB. Defaults remain 4 files, 16 pipelined writes, and 256 KiB packets, for an estimated 16 MiB in-flight write window.

Synchronization scans both local and remote directory trees and builds a typed action plan before mutation. The CLI prints the plan before applying it. Remote-only entries are preserved by default; deletion and file/directory conflict replacement are only permitted when `--delete` is explicitly supplied. `--dry-run` never mutates the remote tree.

Symbolic links encountered during recursive transfer or synchronization scans are currently skipped rather than followed. This prevents accidental traversal outside the requested tree; explicit symlink policy is intentionally separate.

## Fleet resource model

`kssh-fleet` defaults to 8 concurrent SSH tasks and hard-limits concurrency to 64. It retains at most 1 MiB stdout and 1 MiB stderr per in-flight host by default; each stream is capped at 16 MiB and the configured `jobs × output-limit × 2` memory window may not exceed 512 MiB. Output beyond the retention cap is still drained from SSH and marked truncated.

Targets and supported OpenSSH configuration are resolved before the first connection. Checks that inherently require a live transport—host-key verification, authentication, and ProxyCommand expansion-value validation immediately before process spawn—remain fail-closed at runtime and are isolated per host.

## Validation and performance

CI is configured to run checks/tests on Ubuntu, macOS, and Windows, plus a Rust 1.85 MSRV job. The Linux OpenSSH integration workflow starts real `sshd` fixtures and covers authentication, bastions, agent forwarding, RSA signing policy and fail-closed RSA-only host negotiation, SFTP, synchronization, privilege switching, TCP forwarding, typed commands, diagnostics, and fleet execution.

At the time of the v0.7 release-candidate preparation, GitHub Actions successfully creates candidate runs/jobs but the repository's GitHub-hosted jobs are failing before runner allocation: job step lists are empty and logs are not created. Therefore the candidate is **not** claimed to have passed CI, and promotion to `develop`/`main` remains blocked until those jobs actually execute.

The on-demand `Benchmark` workflow records connect/exec latency, large-file SFTP throughput, and recursive small-file transfer timing to a CSV artifact. Performance claims should be based on those measurements rather than configuration alone.

## Branch model

```text
feat/* / fix/* / perf/* / ci/* -> develop -> release/* -> main
```

`develop` is the small-version integration branch. `main` is reserved for stable/major release promotion. Release-candidate integration branches may be used to assemble and validate a large version without bypassing those promotion gates.

## Remaining security-sensitive work

Some features are deliberately not enabled until they can be implemented completely and tested as security boundaries:

- full OpenSSH host-certificate / `@cert-authority` semantics, including CA signature, principals, critical options, host-pattern matching, validity and revocation behavior
- lossless OpenSSH `Include` / `Match` configuration semantics
- encrypted persistent credential storage and its key-management model
- cross-process ControlMaster-style reuse through a local daemon/IPC protocol
- explicit symbolic-link transfer/sync policy

See `docs/ARCHITECTURE.md`, `docs/ENGINEERING.md`, `docs/TUI.md`, `docs/SESSION_WORKSPACE.md`, `docs/FLEET_EXEC.md`, and `SECURITY.md` for design constraints, validation gates, release policy, and invariants.
