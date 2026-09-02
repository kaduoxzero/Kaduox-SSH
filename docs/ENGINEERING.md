# Engineering Kaduox-SSH

Kaduox-SSH is organized around a reusable async Rust core. CLI, future TUI/GUI frontends, and automation should depend on `kaduox-ssh-core` instead of reimplementing SSH protocol behavior.

## Branch and release model

- `main` is the stable release line. It should only receive release-ready major/stable changes.
- `develop` is the integration line for minor releases and ongoing product development.
- `feat/*`, `fix/*`, `perf/*`, and `ci/*` branches start from `develop` and merge back into `develop` after their gates pass.
- Release tags use `v*`; the tag workflow produces GitHub Release binaries for Linux, macOS, and Windows.

## Supported Rust and platforms

The workspace declares Rust 1.85 as its MSRV. CI verifies the workspace against Rust 1.85 and also runs stable Rust checks/tests on Ubuntu, macOS, and Windows.

`Cargo.lock` is committed because Kaduox-SSH ships an application. CI uses `--locked` so dependency resolution is reproducible.

## Validation gates

Normal changes are expected to satisfy:

1. `cargo fmt --all -- --check`
2. `cargo check --workspace --all-targets --locked`
3. `cargo test --workspace --locked`
4. `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
5. `cargo audit`
6. the real OpenSSH integration workflow on Linux

The OpenSSH suite starts isolated `sshd` fixtures and validates public-key auth, exec, sudo privilege switching, ProxyCommand, ProxyJump, Agent authentication, Agent Forwarding, SFTP, privileged staged upload, sync semantics, and local/dynamic/remote forwarding.

## Connection lifecycle

`ConnectionManager` owns named authenticated SSH transports for long-lived frontends. A transport can multiplex independent SSH channels for commands, PTY shells, SFTP sessions, and forwards. The manager:

- reuses a transport by logical connection name;
- never holds its registry lock across DNS, TCP, SSH handshake, or authentication;
- bounds the number of managed connections;
- tracks last use and can prune idle transports;
- can explicitly remove one connection or close the entire registry.

A one-shot `kssh` process still owns only its process-local connection. Cross-process ControlMaster-style reuse would require a local daemon/IPC layer and is intentionally separate from the core manager.

## File-transfer safety

SFTP transfers are streaming and bounded-memory. Recursive operations use file-level concurrency while `russh-sftp` handles request pipelining and server limits.

Uploads can use stable `.kaduox.part` files for resume and staging files for atomic replacement. Privileged uploads do not pretend SFTP can change uid: Kaduox uploads as the authenticated user, then performs an explicit non-interactive sudo install/move.

`sync` plans before applying. Remote-only entries are preserved unless `--delete` is explicitly supplied. Dry-run never mutates the server.

## Host keys and authentication

Host-key policies are `strict`, `accept-new`, and `insecure`. `insecure` is intended for controlled test environments, not routine production use.

Authentication supports private keys, passwords, keyboard-interactive, and SSH agents. Windows authentication tries the OpenSSH Agent named pipe and falls back to Pageant. Agent Forwarding is opt-in.

## Performance work

The on-demand benchmark workflow builds the release profile and runs against a real local OpenSSH server. It records CSV metrics for:

- repeated connect + exec latency;
- large SFTP upload throughput;
- large SFTP download throughput;
- recursive small-file upload elapsed time.

Use benchmark results to justify tuning; do not claim a performance improvement from configuration changes alone.

The release profile uses optimized code generation, thin LTO, one codegen unit, and stripped symbols. Keep protocol buffers bounded and prefer concurrency over whole-file buffering.

## Release process

A stable release should first be integrated and validated on `develop`, then promoted according to the repository release policy. Pushing a `v*` tag triggers release packaging. The workflow builds `kssh` with the committed lockfile and uploads platform binaries to a GitHub Release.

Do not promote an unverified development branch directly to `main`.

## Security-sensitive future work

Changes to host-certificate trust, `known_hosts` parsing, authentication, agent forwarding, sudo behavior, and destructive sync semantics require dedicated tests. In particular, OpenSSH host certificates and `@cert-authority` semantics should only be enabled when CA signature, validity, principal matching, critical options, hostname patterns, and revocation behavior are all handled correctly; partial certificate validation is not acceptable.
