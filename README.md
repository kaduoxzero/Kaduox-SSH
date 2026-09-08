# Kaduox-SSH

**English** | [简体中文](README.zh-CN.md)

## Windows desktop preview

[Download the Windows desktop prerelease](https://github.com/kaduoxzero/Kaduox-SSH/releases) · [Desktop user guide (中文)](docs/DESKTOP_GUIDE.zh-CN.md) · [Agent / MCP setup](docs/MCP.md)

The v0.33.0-rc.1 desktop preview has an install-directory picker, light/dark themes, independent terminal tabs, user-managed host folders, up to five SSH jump servers, metrics dashboards, SFTP, forwarding and a configurable AI provider. No personal host configuration is bundled. This manually built Windows preview is **unsigned**; the stable cross-platform release pipeline described below is not a claim that this preview is signed or CI-qualified.

Kaduox-SSH is a Rust-first SSH client focused on long-term maintainability, low latency, bounded memory usage, reproducible builds, and a reusable core shared by CLI, TUI, inventory, and fleet frontends.

## Current capabilities

- SSH connection, command execution, and interactive PTY shell
- OpenSSH `~/.ssh/config` host resolution with bounded user-config `Include` expansion (including single-pass `${ENV}`, literal `%%`, and native `%l`/`%L` local-hostname paths), supported `Match all` / `Match originalhost` evaluation, and fail-closed handling for unsupported structural semantics
- cross-platform OpenSSH user-config trust enforcement: Unix uid/mode checks and Windows owner/DACL checks, both tied to the same opened file used for parsing
- target-scoped OpenSSH Host Certificate verification through matching clear-text or OpenSSH-hashed `known_hosts` `@cert-authority` entries, with principal/validity/critical-option/revocation checks
- clear-text and OpenSSH-hashed `@revoked` enforcement for ordinary host keys, certificate subject keys, and certificate signing CAs
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
- explicit recursive transfer/sync symbolic-link policy with `skip` and fail-closed `reject` modes; links are never followed
- reusable in-process authenticated connection manager with explicit long-lived leases
- `kssh-tui` multi-host Session Dashboard with persistent authenticated sessions
- TUI OpenSSH Host picker, remote SFTP workspace, interactive shell/sudo shell, tracked transfers, and remote file actions
- bounded Dashboard Broadcast across already-open authenticated sessions
- `kssh-fleet` bounded-concurrency typed command execution across multiple SSH targets
- per-target fleet failure isolation, capped output retention, and terminal-safe aggregation
- `kssh-inventory` offline inventory validation, listing, and group expansion
- persistent private host library with host aliases, recent-use ordering, and named jump chains (`kssh hosts`, `kssh chains`)
- `kssh-daemon` per-user local IPC connection-reuse daemon; `kssh` reuses authenticated transports through it before falling back to a direct connection, with per-hop interactive jump authentication bridged over the private IPC channel
- optional login-password persistence in the operating system credential store (Windows Credential Manager, macOS Keychain, Linux Secret Service) via `kssh credentials set/check/delete`; Kaduox-SSH never writes passwords to its own files
- shell completion scripts for bash/zsh/fish/powershell/elvish via `kssh completions <shell>`
- native Tauri Windows desktop client with light/dark themes, terminal/SFTP/forwarding workspaces, host inspection, built-in AI assistant, and an optional OpenAI-compatible provider
- `kaduox-ssh-mcp` local stdio MCP server for Agent integration; read-only host/route/basic-info/SFTP tools are enabled by default, while remote exec and transfers require explicit environment switches
- real OpenSSH protocol integration fixtures, including fleet and Host Certificate coverage
- Linux/macOS/Windows CI, Rust 1.85 MSRV, Clippy, audit, release-policy, and real-OpenSSH workflow gates
- committed `Cargo.lock` with `--locked` builds
- on-demand real-OpenSSH performance benchmark harness
- deterministic four-suite release packaging with per-binary manifests and SHA-256 archive checksums
- stable Windows Authenticode signing plus macOS Developer ID signing/notarization
- four target-specific SPDX 2.3 SBOMs with stable-release provenance and archive-to-SBOM attestation policy

The authenticated SSH login user cannot be changed after SSH authentication. Kaduox-SSH opens additional channels on the existing transport and uses `sudo -iu <user>` for interactive privilege switching or `sudo -n -u <user> -- sh -lc ...` for commands.

Privileged upload does not pretend SFTP can change uid. Kaduox-SSH stages files or directory trees as the SSH login user, then performs an explicit sudo-backed install phase as the requested remote OS user and removes the staging data afterwards. Recursive privileged replacement uses a sibling work directory and a remove-then-rename step when the destination already exists, so replacement of an existing directory is deliberately not claimed to be atomic.

## Frontends

Kaduox-SSH currently ships four binaries over the same core security and transport implementation:

- `kssh`: single-target CLI for shell, exec, transfer, sync, forwarding, inspection, and diagnostics;
- `kssh-tui`: multi-host interactive Session Dashboard. Each open host owns an explicit `ConnectionLease`; entering/leaving its remote workspace does not reconnect. The dashboard can also broadcast one operator-authored command over all already-open sessions;
- `kssh-fleet`: non-interactive bounded-concurrency command execution over explicit targets or inventory groups. It preflights target/config/typed-command input before opening the first SSH connection and isolates runtime failures per host;
- `kssh-inventory`: offline inventory validation, host/group listing, and deterministic nested-group expansion.

The TUI and fleet frontends do not implement separate SSH stacks. They reuse `kaduox-ssh-core` for authentication, host-key policy, ProxyJump/ProxyCommand, channels, SFTP, privilege switching, and transport limits.

See `docs/TUI.md`, `docs/SESSION_WORKSPACE.md`, `docs/FLEET_EXEC.md`, `docs/INVENTORY.md`, `docs/OPENSSH_CONFIG.md`, and `docs/HOST_CERTIFICATES.md` for frontend/config/trust-specific contracts and resource limits.

See `docs/MCP.md` for the local MCP server configuration, tool list, and permission switches.

## RSA security policy

Kaduox-SSH intentionally disables Russh's optional local RSA signer while the dependency path is affected by `RUSTSEC-2023-0071` (the Marvin timing attack advisory). The affected `rsa` crate is therefore absent from the resolved application lockfile.

This policy distinguishes private-key signing from public-key compatibility:

- Ed25519 and ECDSA private-key files can be used directly.
- A local RSA private-key file fails closed with an explicit remediation message.
- RSA user authentication remains available through an external SSH agent because the private-key operation stays inside the agent rather than Kaduox-SSH.
- RSA-only server host keys are currently rejected during key-exchange negotiation. In Russh 0.63.1 the feature needed for RSA host-signature verification is coupled to the affected local RSA signer dependency, so Kaduox-SSH prefers a fail-closed compatibility tradeoff over advertising an algorithm it cannot safely verify. Servers should expose Ed25519 or ECDSA host keys.
- RSA Host Certificate variants are not advertised. v0.16's certificate fixture validates an Ed25519 CA and Ed25519 certified host key; RSA CA/certificate-signature compatibility is not claimed while Russh's optional RSA feature remains disabled.

The real OpenSSH integration suite is designed to verify direct RSA-key rejection, agent-backed RSA authentication, early negotiation failure against an RSA-host-key-only `sshd` fixture, and the v0.16 Host Certificate trust path. Do not re-enable the Russh `rsa` feature until the advisory has an acceptable upstream resolution and the full security test suite executes green.

## OpenSSH config and host trust compatibility

Kaduox-SSH resolves supported OpenSSH settings such as `HostName`, `User`, `Port`, `IdentityFile`, `UserKnownHostsFile`, `ProxyCommand`, and `ProxyJump` from `~/.ssh/config`.

v0.14 adds bounded `Include` resolution before the downstream host parser. The supported subset includes global or `Host`-scoped includes, multiple/quoted paths, absolute paths, paths relative to `~/.ssh`, current-user `~/...`, `*` and `?` wildcards, lexical processing order, nested includes, OpenSSH-style hidden-file matching, and restoration of the containing scope after every included file. The OpenSSH Host catalog uses the same include graph, so concrete aliases in included files are visible to the TUI picker.

v0.19 adds a deliberately bounded `Match` subset for connection resolution: standalone `Match all` and a single `Match originalhost <pattern-list>` criterion. `originalhost` is evaluated against the lookup alias before `HostName` rewriting; its pattern list is ASCII case-insensitive, comma-separated, supports `*`/`?`, and honors `!` negation. Other Match criteria and combinations remain fail-closed.

v0.28 integrates that Match subset with the Include scope machine. Includes under active Host/Match scopes are evaluated normally and the caller's active state is restored after each included file. An Include reached from an inactive Host/Match scope is still opened, trust-checked, cycle/resource checked, and syntax-validated, but child Host/Match blocks cannot reactivate configuration for the target. This closes the earlier scope-reentry gap where an included `Host <target>` could otherwise escape an inactive parent block.

v0.29 adds OpenSSH-style `${NAME}` environment expansion for Include paths. Expansion is single-pass: bytes produced by an environment value are not rescanned as nested `${...}` expressions or percent tokens. Missing, empty, unterminated, or non-UTF-8 environment values fail closed, and the expanded result remains subject to the existing 16 KiB path limit and all normal Include trust/resource checks.

v0.30 adds the context-free OpenSSH `%%` escape for a literal percent in Include paths. `%d` is deliberately not approximated with the selected config home because OpenSSH derives `%d` from the local passwd entry's `pw_dir`, while Kaduox-SSH currently locates its config home from platform environment variables; those values can differ.

v0.31 adds OpenSSH `%l` and `%L` Include expansion from the platform-native local hostname. `%l` uses the full hostname returned by `gethostname`; `%L` uses the same per-argument hostname snapshot truncated at the first dot. Unix calls `gethostname` directly. Windows initializes Winsock 2.2 once per process before using Winsock `gethostname`, following Rust std's socket initialization lifetime rather than repeatedly starting and cleaning up Winsock. The hostname value itself is not cached process-wide. Hostname lookup/non-UTF-8 failures remain fail-closed, and `${ENV}` substitution bytes are still never rescanned as percent tokens.

v0.15 applies one trust boundary to the root user config and every nested Include. On Unix, the opened file must be regular, owned by the process real uid or root, and not writable by group/other. Metadata verification and parsing bytes are tied to the same opened file descriptor, removing the path-based stat/read TOCTOU window. v0.26 adds the Windows equivalent boundary: the owner/DACL is queried from the already-open file handle, untrusted write-capable Allow ACEs are rejected, read-only access for other principals remains allowed, and advanced allow ACE layouts fail closed instead of being approximated.

v0.16 adds fail-closed Host Certificate and revocation semantics. Certificate algorithms remain disabled unless the specific final target or ProxyJump hop has a matching `@cert-authority`. Marker host patterns may be clear-text or OpenSSH `|1|base64-salt|base64-hmac-sha1` hashed names. A certificate must be a Host certificate, have a trusted signing CA, valid signature/time window, an acceptable hostname principal, no unsupported critical options, and neither its subject key nor CA may be `@revoked`. Ordinary host keys are also checked against applicable `@revoked` markers before known-hosts acceptance or explicit insecure acceptance. A failed certificate is never downgraded to its embedded ordinary key.

Configuration/trust behavior remains fail-closed where OpenSSH semantics are not yet reproduced exactly:

- A missing `~/.ssh/config` is normal and falls back to direct/default connection settings.
- Read, trust, expansion, and parse errors in an existing config are returned to the caller; they are never silently converted into a direct connection.
- `Match all` and single-criterion `Match originalhost <pattern-list>` are supported. `canonical`, `final`, `exec`, `localnetwork`, `host`, `tagged`, `command`, `user`, `localuser`, `version`, combined criteria, criterion negation/`criterion=value`, and quoted/escaped Match arguments remain explicitly rejected.
- Include `%%`, `%l`, and `%L` are supported. The remaining named percent tokens (`%C`, `%d`, `%h`, `%k`, `%n`, `%p`, `%r`, `%u`, `%i`, `%j`), `~other-user` expansion, and full bracket/collation glob expressions are explicitly rejected instead of being approximated.
- Include expansion is capped at 16 nesting levels, 256 processed files, a 4 MiB config budget, 64 paths per directive, 16 KiB per include path after environment/percent expansion, and 1024 bytes per wildcard component; recursive include cycles fail explicitly.
- Hashed marker names are exact HMAC-SHA1 matches over the effective known-hosts target bytes; they are not case-folded wildcard patterns. Non-default ports use OpenSSH's `[host]:port` target form.
- Host Certificate critical options are not interpreted; any critical option rejects the certificate. RSA Host Certificates and RSA CA/certificate-signature compatibility are not claimed while the RSA feature remains disabled.
- Windows SIDHistory name-equivalence remains intentionally stricter than Win32-OpenSSH: distinct SIDs remain distinct principals instead of gaining trust from reverse account-name equivalence.
- The upstream parser exposes `StrictHostKeyChecking` only as a boolean. Values other than `no` therefore lose their exact OpenSSH policy. Use Kaduox-SSH's explicit `--host-key strict`, `--host-key accept-new`, or `--host-key insecure` when exact behavior matters.

See `docs/OPENSSH_CONFIG.md` and `docs/HOST_CERTIFICATES.md` for the exact supported and rejected forms.

## Architecture

```text
crates/
  kaduox-ssh-core/    # transport/auth/session/forward/SFTP/sync/privilege/manager core
  kaduox-ssh-hosts/   # atomic private TOML host library, aliases, and named jump chains
  kaduox-ssh-daemon/  # per-user IPC connection-reuse daemon (peer identity, bounded protocol)
  kaduox-ssh-cli/     # kssh + kssh-tui + kssh-fleet + kssh-inventory package
```

The core is frontend-independent. TUI sessions use explicit connection leases over the same manager, while fleet operations use the same typed command and transport APIs with separate batch scheduling and output policies.

## Build

Rust 1.85+ is the declared MSRV.

```bash
cargo build --release --locked
```

The resulting binaries are `kssh`, `kssh-tui`, `kssh-fleet`, and `kssh-inventory`.

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

# Recursive directory transfers; reject any symbolic link instead of skipping it
kssh server.example.com upload ./dist /srv/www/dist -r --jobs 8 --symlinks reject
kssh server.example.com download /srv/logs ./logs -r --jobs 8 --symlinks reject

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

# Reject symbolic links during synchronization instead of the default skip policy
kssh server.example.com sync ./dist /srv/www/dist --symlinks reject

# Open the multi-host TUI dashboard
kssh-tui production

# Run one typed command across a bounded fleet
kssh-fleet -H web-01 -H web-02 -H deploy@web-03 --jobs 3 -- uname -a

# Validate inventory without connecting
kssh-inventory check
```

By default host keys use `accept-new`: unknown ordinary keys are written to the normal OpenSSH `known_hosts` file while changed keys are rejected. Use `--host-key strict` to require a pre-existing ordinary key or a matching trusted Host Certificate CA. `--host-key insecure` is intentionally explicit and should only be used for disposable/test environments; applicable `@revoked` entries still reject the key.

## Transfer and sync design

Transfers use bounded buffers and leave large-file request pipelining to `russh-sftp`, while Kaduox-SSH controls higher-level policy such as stable resume files, atomic finalization, directory concurrency, progress, cancellation, and privileged staging. SFTP session limits are configurable locally and are still constrained by limits negotiated with the remote server.

Transfer tuning is fail-closed rather than unbounded: file concurrency is limited to 128, pipelined SFTP writes to 128, packet size to 4 KiB–4 MiB, and the estimated `file_concurrency × write_concurrency × packet_size` window must stay at or below 512 MiB. Defaults remain 4 files, 16 pipelined writes, and 256 KiB packets, for an estimated 16 MiB in-flight write window.

Synchronization scans both local and remote directory trees and builds a typed action plan before mutation. The CLI prints the plan before applying it. Remote-only entries are preserved by default; deletion and file/directory conflict replacement are only permitted when `--delete` is explicitly supplied. `--dry-run` never mutates the remote tree.

Recursive upload/download and synchronization expose an explicit symbolic-link policy. `skip` is the compatibility default and never follows a symbolic link/reparse point; `reject` performs a fail-closed preflight and aborts when a link-like entry is encountered. Follow/preserve semantics remain intentionally unsupported because they require bounded cycle/escape handling and complete cross-platform target encoding.

## Fleet resource model

`kssh-fleet` defaults to 8 concurrent SSH tasks and hard-limits concurrency to 64. It retains at most 1 MiB stdout and 1 MiB stderr per in-flight host by default; each stream is capped at 16 MiB and the configured `jobs × output-limit × 2` memory window may not exceed 512 MiB. Output beyond the retention cap is still drained from SSH and marked truncated.

Targets and supported OpenSSH configuration are resolved before the first connection. Checks that inherently require a live transport—host-key/certificate verification, authentication, and ProxyCommand expansion-value validation immediately before process spawn—remain fail-closed at runtime and are isolated per host.

## Validation and performance

CI is configured to run checks/tests on Ubuntu, macOS, and Windows, plus a Rust 1.85 MSRV job. The Linux OpenSSH integration workflow starts real `sshd` fixtures and covers authentication, bastions, agent forwarding, RSA signing policy and fail-closed RSA-only host negotiation, Host Certificate trust/revocation, SFTP, synchronization, privilege switching, TCP forwarding, typed commands, diagnostics, and fleet execution. Quality also gates Clippy, dependency audit, and release-policy tests.

The repository's GitHub-hosted jobs are currently failing before any workflow step executes (`steps=null` and no usable job logs). Therefore candidate branches are **not** claimed to have passed CI, and promotion to `develop`/`main` remains blocked until those jobs actually acquire runners and execute successfully.

Stable releases require native Windows/macOS signing and now produce four target-specific SPDX files. Stable publication also requires the four-target GitHub attestation matrix; pre-releases may opt into the same attestation path. Published-release qualification verifies the eight checksummed primary assets, target-SBOM identity, native signatures, packaged binary versions, and the real Linux OpenSSH path.

The on-demand `Benchmark` workflow records connect/exec latency, large-file SFTP throughput, and recursive small-file transfer timing to a CSV artifact. Performance claims should be based on those measurements rather than configuration alone.

## Branch model

```text
feat/* / fix/* / perf/* / ci/* -> develop -> release/* -> main
```

`develop` is the small-version integration branch. `main` is reserved for stable/major release promotion. Release-candidate integration branches may be used to assemble and validate a large version without bypassing those promotion gates.

## Remaining security-sensitive work

Some features are deliberately not enabled until they can be implemented completely and tested as security boundaries:

- complete OpenSSH `Match` evaluation beyond the bounded `all` / `originalhost` subset, plus the remaining named Include percent tokens/`~user`/full-glob semantics;
- RSA Host Certificate/CA compatibility if the RSA dependency path becomes safe;
- Kaduox-SSH-managed encrypted credential files with a custom key-management model (the OS credential store integration covers the common persistence case);
- symbolic-link follow/preserve transfer/sync semantics with bounded cycle, escape, and cross-platform target handling.

See `docs/ARCHITECTURE.md`, `docs/ENGINEERING.md`, `docs/OPENSSH_CONFIG.md`, `docs/HOST_CERTIFICATES.md`, `docs/RELEASE.md`, `docs/TUI.md`, `docs/SESSION_WORKSPACE.md`, `docs/FLEET_EXEC.md`, and `SECURITY.md` for design constraints, validation gates, release policy, and invariants.
