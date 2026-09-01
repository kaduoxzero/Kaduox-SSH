# Kaduox-SSH

Kaduox-SSH is a Rust-first SSH client focused on long-term maintainability, low latency, bounded memory usage, and a reusable core that can later power CLI, TUI, and GUI frontends.

## v0.1 scope

- SSH connection and command execution
- Interactive PTY shell
- SFTP upload/download using bounded streaming buffers
- Fast privilege switching on an existing SSH transport through `sudo`
- OpenSSH agent authentication through `SSH_AUTH_SOCK` on Unix
- `known_hosts` verification with strict, accept-new, and explicit insecure modes

The authenticated SSH user itself cannot be changed after SSH authentication. Kaduox-SSH therefore implements privilege switching by opening a new channel on the existing transport and starting `sudo -iu <user>` for an interactive shell, or `sudo -n -u <user> -- sh -lc ...` for non-interactive commands.

## Architecture

```text
crates/
  kaduox-ssh-core/   # SSH/SFTP/auth/session/privilege core; no CLI policy
  kaduox-ssh-cli/    # current CLI frontend
```

The core is intentionally independent from a particular terminal UI so a future desktop UI or TUI can reuse the same connection and transfer engine.

## Build

Rust 1.85+ is required.

```bash
cargo build --release
```

The CLI binary is named `kssh`.

## Examples

Agent authentication is the default when neither `--identity` nor `--password` is supplied.

```bash
# Interactive shell
kssh server.example.com --user deploy shell

# Open a root login shell over the same SSH transport
kssh server.example.com --user deploy shell --as-user root

# Execute a command
kssh server.example.com --user deploy exec -- uname -a

# Execute as another remote user (non-interactive sudo)
kssh server.example.com --user deploy exec --as-user root -- id

# Private key authentication
kssh server.example.com --user deploy --identity ~/.ssh/id_ed25519 shell

# Password authentication (password is prompted, not accepted as a CLI value)
kssh server.example.com --user deploy --password shell

# SFTP transfer
kssh server.example.com --user deploy upload ./app.tar.zst /tmp/app.tar.zst
kssh server.example.com --user deploy download /var/log/app.log ./app.log
```

By default, host keys use `accept-new`: unknown keys are written to the normal OpenSSH `known_hosts` file, while changed keys are rejected. Use `--host-key strict` to require a pre-existing entry. `--host-key insecure` is intentionally explicit and should only be used for disposable/test environments.

## Performance direction

Kaduox-SSH uses the Tokio + Russh asynchronous stack. File transfers are streamed with a fixed 255 KiB buffer instead of loading an entire file into memory. The next transfer milestone is bounded request pipelining, adaptive concurrency, and benchmark coverage for high-BDP links and large batches of small files.

## Roadmap

### v0.2

- OpenSSH config import (`~/.ssh/config`)
- ProxyJump / bastion chains
- local / remote / dynamic port forwarding
- agent forwarding
- keyboard-interactive authentication
- resumable and recursive SFTP transfers
- transfer progress/events and cancellation
- connection profiles
- terminal resize propagation

### v0.3+

- connection pool and multiplexed session manager
- terminal state/event API suitable for TUI/GUI
- cross-platform agent adapters (Windows OpenSSH agent / Pageant)
- encrypted credential/profile storage
- SCP compatibility where required
- benchmarks, flamegraphs, fuzzing, protocol integration tests

See `docs/ARCHITECTURE.md` for design constraints and invariants.
