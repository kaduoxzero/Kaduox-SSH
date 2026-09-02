# Kaduox-SSH

Kaduox-SSH is a Rust-first SSH client focused on long-term maintainability, low latency, bounded memory usage, and a reusable core that can power CLI, TUI, and GUI frontends.

## Current capabilities

- SSH connection, command execution, and interactive PTY shell
- OpenSSH `~/.ssh/config` resolution
- ProxyJump chains and ProxyCommand transports
- password, private-key, keyboard-interactive, OpenSSH-agent, and Pageant/Windows-agent authentication paths
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

The authenticated SSH login user cannot be changed after SSH authentication. Kaduox-SSH opens additional channels on the existing transport and uses `sudo -iu <user>` for interactive privilege switching or `sudo -n -u <user> -- sh -lc ...` for commands.

Privileged file upload does not pretend SFTP can change uid. Kaduox-SSH uploads a temporary file as the SSH login user, performs an explicit `sudo install`/move to the privileged destination, and then removes the staging file.

## Architecture

```text
crates/
  kaduox-ssh-core/   # transport/auth/session/forward/SFTP/privilege core
  kaduox-ssh-cli/    # current CLI frontend
```

The core is intentionally independent from a particular terminal UI so future desktop and TUI frontends can reuse the same connection and transfer engines.

## Build

Rust 1.85+ is required.

```bash
cargo build --release
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

# Private key / password
kssh server.example.com --user deploy --identity ~/.ssh/id_ed25519 shell
kssh server.example.com --user deploy --password shell

# ProxyJump and forwarding
kssh target.internal -J bastion.example -L 8080:127.0.0.1:80 tunnel
kssh target.internal -D 1080 tunnel

# Atomic single-file upload/download
kssh server.example.com upload ./app.tar.zst /tmp/app.tar.zst
kssh server.example.com download /var/log/app.log ./app.log

# Resume an interrupted transfer
kssh server.example.com upload ./large.img /srv/large.img --resume
kssh server.example.com download /srv/large.img ./large.img --resume

# Recursive directory transfers, four files at a time by default
kssh server.example.com upload ./dist /srv/www/dist -r --jobs 8
kssh server.example.com download /srv/logs ./logs -r --jobs 8

# Install a staged upload as root, without running SFTP as root
kssh server.example.com upload ./nginx.conf /etc/nginx/nginx.conf --as-user root --mode 0644
```

By default host keys use `accept-new`: unknown keys are written to the normal OpenSSH `known_hosts` file while changed keys are rejected. Use `--host-key strict` to require a pre-existing entry. `--host-key insecure` is intentionally explicit and should only be used for disposable/test environments.

## Transfer design

Transfers use bounded buffers and leave large-file pipelining to `russh-sftp`, while Kaduox-SSH controls higher-level policy such as stable resume files, atomic finalization, directory concurrency, progress, cancellation, and privileged staging. SFTP session limits are configurable locally and are still constrained by limits negotiated with the remote server.

Symbolic links encountered during recursive transfers are currently skipped rather than followed. This prevents accidental traversal outside the requested tree; explicit symlink policy will be added separately.

## Branch model

```text
feat/* / fix/* / perf/* -> develop -> release/* -> main
```

`develop` is the small-version integration branch. `main` is reserved for stable/major release promotion.

## Roadmap

### v0.2 follow-up

- mirror/sync mode with explicit dry-run and delete policy
- richer transfer verification/checksum policy
- connection profiles
- integration tests against real OpenSSH servers

### v0.3+

- connection pool and multiplexed session manager
- terminal state/event API suitable for TUI/GUI
- encrypted credential/profile storage
- SCP compatibility where required
- benchmarks, flamegraphs, fuzzing, and protocol integration tests

See `docs/ARCHITECTURE.md` for design constraints and invariants.
