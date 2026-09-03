# Kaduox-SSH

**English** | [简体中文](README.zh-CN.md)

Kaduox-SSH is a Rust-first SSH client focused on long-term maintainability, low latency, bounded memory usage, reproducible builds, and a reusable core that can power CLI, TUI, and GUI frontends.

## Current capabilities

- SSH connection, command execution, and interactive PTY shell
- OpenSSH `~/.ssh/config` resolution
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
- sudo-backed privileged single-file upload using unprivileged SFTP staging
- SFTP-native local-to-remote directory synchronization with non-mutating planning
- explicit `--delete` policy for destructive sync operations
- reusable in-process authenticated connection manager for future long-lived TUI/GUI frontends
- real OpenSSH protocol integration tests
- Linux/macOS/Windows CI plus a verified Rust 1.85 MSRV gate
- committed `Cargo.lock` with `--locked` CI builds
- clippy `-D warnings` and dependency audit quality gates
- on-demand real-OpenSSH performance benchmark harness
- tag-driven Linux/macOS/Windows release packaging

The authenticated SSH login user cannot be changed after SSH authentication. Kaduox-SSH opens additional channels on the existing transport and uses `sudo -iu <user>` for interactive privilege switching or `sudo -n -u <user> -- sh -lc ...` for commands.

Privileged file upload does not pretend SFTP can change uid. Kaduox-SSH uploads a temporary file as the SSH login user, performs an explicit `sudo install`/move to the privileged destination, and then removes the staging file.

## RSA security policy

Kaduox-SSH intentionally disables Russh's optional local RSA signer while the dependency path is affected by `RUSTSEC-2023-0071` (the Marvin timing attack advisory). The affected `rsa` crate is therefore absent from the resolved application lockfile.

This policy distinguishes private-key signing from public-key compatibility:

- Ed25519 and ECDSA private-key files can be used directly.
- A local RSA private-key file fails closed with an explicit remediation message.
- RSA user authentication remains available through an external SSH agent because the private-key operation stays inside the agent rather than Kaduox-SSH.
- RSA-only server host keys are currently rejected during key-exchange negotiation. In Russh 0.63.1 the feature needed for RSA host-signature verification is coupled to the affected local RSA signer dependency, so Kaduox-SSH prefers a fail-closed compatibility tradeoff over advertising an algorithm it cannot safely verify. Servers should expose Ed25519 or ECDSA host keys.

The real OpenSSH integration suite continuously verifies direct RSA-key rejection, agent-backed RSA authentication, and early negotiation failure against an RSA-host-key-only `sshd` fixture. Do not re-enable the Russh `rsa` feature until the advisory has an acceptable upstream resolution and the full security test suite remains green.

## Architecture

```text
crates/
  kaduox-ssh-core/   # transport/auth/session/forward/SFTP/sync/privilege/manager core
  kaduox-ssh-cli/    # current CLI frontend
```

The core is intentionally independent from a particular terminal UI so future desktop and TUI frontends can reuse the same connection, forwarding, and transfer engines.

## Build

Rust 1.85+ is required and continuously checked as the declared MSRV.

```bash
cargo build --release --locked
```

The CLI binary is named `kssh`.

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

# Inspect a sync plan without modifying the server
kssh server.example.com sync ./dist /srv/www/dist --dry-run

# Apply non-destructive synchronization
kssh server.example.com sync ./dist /srv/www/dist

# Mirror the local tree, explicitly allowing deletion of remote-only entries
kssh server.example.com sync ./dist /srv/www/dist --delete

# Faster comparisons when timestamps are unreliable
kssh server.example.com sync ./dist /srv/www/dist --size-only --jobs 8
```

By default host keys use `accept-new`: unknown keys are written to the normal OpenSSH `known_hosts` file while changed keys are rejected. Use `--host-key strict` to require a pre-existing entry. `--host-key insecure` is intentionally explicit and should only be used for disposable/test environments.

## Transfer and sync design

Transfers use bounded buffers and leave large-file request pipelining to `russh-sftp`, while Kaduox-SSH controls higher-level policy such as stable resume files, atomic finalization, directory concurrency, progress, cancellation, and privileged staging. SFTP session limits are configurable locally and are still constrained by limits negotiated with the remote server.

Transfer tuning is fail-closed rather than unbounded: file concurrency is limited to 128, pipelined SFTP writes to 128, packet size to 4 KiB–4 MiB, and the estimated `file_concurrency × write_concurrency × packet_size` window must stay at or below 512 MiB. Defaults remain 4 files, 16 pipelined writes, and 256 KiB packets, for an estimated 16 MiB in-flight write window.

Synchronization scans both local and remote directory trees and builds a typed action plan before mutation. The CLI prints the plan before applying it. Remote-only entries are preserved by default; deletion and file/directory conflict replacement are only permitted when `--delete` is explicitly supplied. `--dry-run` never mutates the remote tree.

Symbolic links encountered during recursive transfer or synchronization scans are currently skipped rather than followed. This prevents accidental traversal outside the requested tree; explicit symlink policy is intentionally separate.

## Validation and performance

Normal CI runs checks and tests on Ubuntu, macOS, and Windows, and separately verifies Rust 1.85. The Linux OpenSSH integration workflow starts real `sshd` fixtures and exercises authentication, bastions, agent forwarding, RSA signing policy and fail-closed RSA-only host negotiation, SFTP, synchronization, privilege switching, and TCP forwarding.

The on-demand `Benchmark` workflow records connect/exec latency, large-file SFTP throughput, and recursive small-file transfer timing to a CSV artifact. Performance claims should be based on those measurements rather than configuration alone.

## Branch model

```text
feat/* / fix/* / perf/* / ci/* -> develop -> release/* -> main
```

`develop` is the small-version integration branch. `main` is reserved for stable/major release promotion.

## Remaining security-sensitive work

Some features are deliberately not enabled until they can be implemented completely and tested as security boundaries:

- full OpenSSH host-certificate / `@cert-authority` semantics, including CA signature, principals, critical options, host-pattern matching, validity and revocation behavior
- encrypted persistent credential storage and its key-management model
- cross-process ControlMaster-style reuse through a local daemon/IPC protocol
- explicit symbolic-link transfer/sync policy

See `docs/ARCHITECTURE.md`, `docs/ENGINEERING.md`, and `SECURITY.md` for design constraints, validation gates, release policy, and invariants.