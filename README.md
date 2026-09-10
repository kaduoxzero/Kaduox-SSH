# Kaduox-SSH

**English** | [简体中文](README.zh-CN.md)

<img src="apps/kaduox-desktop/public/app-icon.png" width="72" height="72" alt="Kaduox SSH icon">

Kaduox-SSH is a Windows desktop SSH client and Rust toolkit for terminal sessions, remote file management, SSH jump chains, system metrics, port forwarding, multi-host operations, and Agent integration.

The desktop application is a native Tauri client with an embedded local UI. It does **not** require a separately deployed web service.

[Download v0.33.0-rc.11](https://github.com/kaduoxzero/Kaduox-SSH/releases/tag/v0.33.0-rc.11) · [Desktop guide (中文)](docs/DESKTOP_GUIDE.zh-CN.md) · [MCP / Agent setup](docs/MCP.md) · [Release notes](docs/releases/v0.33.0-rc.11.md)

## Download and install

The current prerelease provides **Windows x64** builds. Linux and macOS desktop packages are not included. These preview binaries are manually built and **not Authenticode-signed**, so Windows may show an unknown-publisher warning.

| Download | Use it for |
| --- | --- |
| [Windows installer](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.11/Kaduox-SSH-0.33.0-rc.11-windows-x64-setup.exe) | Recommended for most users. Lets you choose the install directory and installer language. Includes the WebView2 offline installation component. |
| [Standalone desktop EXE](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.11/Kaduox-SSH-0.33.0-rc.11-windows-x64.exe) | Runs without installation. WebView2 Runtime must already be installed. |
| [CLI / TUI tools ZIP](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.11/Kaduox-SSH-0.33.0-rc.11-windows-x64-tools.zip) | Contains `kssh`, `kssh-tui`, `kssh-fleet`, and `kssh-inventory`. |
| [MCP server EXE](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.11/kaduox-ssh-mcp.exe) | Local stdio MCP server for compatible Agents. |
| [SHA256SUMS.txt](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.11/SHA256SUMS.txt) | Verifies downloaded files, for example with `Get-FileHash -Algorithm SHA256 <file>`. |

No personal servers, passwords, API keys, or user-data files are bundled. A fresh profile starts with an empty host list. User passwords and AI API keys are stored through the operating-system credential store rather than in Kaduox-SSH data files.

## Desktop features

### Hosts and terminals

- Save hosts with display name, address, SSH port, login user, tags, authentication, and optional jump-chain settings.
- Host names support Chinese characters, spaces, and parentheses.
- Organize target hosts and dedicated jump servers into folders.
- Open multiple SSH sessions and independent terminal tabs.
- Reopening an already connected host switches to its existing session instead of creating a duplicate host.
- Terminal commands entered through the app are recorded in persistent per-host history. Run history can be filtered by machine and day and copied for reuse.
- Terminal process state is not restored after the desktop client exits.

### SFTP file manager

The desktop SFTP workspace supports:

- remote directory browsing;
- file upload and download;
- recursive remote-folder download with bounded safety limits;
- create file / create directory;
- rename and delete operations;
- properties and name search;
- in-place editing of small text files;
- persistent file-operation audit entries in run history.

Transfer operations use bounded-memory SFTP primitives in the shared Rust core. The CLI additionally supports recursive transfers, resumable `.kaduox.part` files, atomic staging, cancellation, synchronization planning, and explicit symlink policies.

### System information

The Info workspace shows available remote CPU, memory, disk, network, and GPU metrics. Data is sampled immediately when the view opens and then refreshed every **10 seconds** while the view remains active.

### SSH jump chains

Configure jump servers on the **final target host**. A normal route looks like:

```text
Your computer -> Jump A -> Jump B -> Target
```

Kaduox-SSH supports **up to five jump servers plus one final target**. Each hop uses its own SSH host, port, login user, and authentication. Changing a route requires reconnecting the target session.

### Port forwarding

Supported forwarding modes:

| Mode | Direction |
| --- | --- |
| Local `-L` | Local listener -> SSH tunnel -> service reachable from the remote host |
| Remote `-R` | Remote listener -> SSH tunnel -> service reachable from your computer |
| Dynamic `-D` | Local SOCKS5 listener -> SSH tunnel -> remote network |

Port forwarding transports TCP traffic. It does not replace HTTP routing, TLS termination, reverse proxies, firewalls, or application authentication.

### Host identity and credentials

Host-key verification modes are:

- **Strict**: the host key must already be trusted.
- **Accept new**: the first key is recorded; later changes are rejected.
- **Insecure**: skips ordinary host identity verification and is intended only for isolated testing.

Matching revoked keys are still rejected. See the [desktop guide](docs/DESKTOP_GUIDE.zh-CN.md) and [security policy](SECURITY.md) for trust and credential details.

## Kaduox AI

Kaduox AI is an operations assistant built into the desktop client. It connects only to an **OpenAI-compatible provider that you configure**. Configure the provider name, API endpoint, API key, and model; supported providers can expose a model list, or a model can be entered manually.

There is **no offline answer mode**. Remote provider endpoints must use HTTPS; loopback endpoints may use HTTP. API keys are stored in the OS credential store.

AI conversations can be scoped to the selected machine, and host context is sent only when enabled. Current command execution uses explicit safety modes:

| Mode | Behavior |
| --- | --- |
| Approval | Read-only commands may run automatically. Modifying or deleting commands require approval. Dangerous commands are refused. |
| Full | Modifying commands may run automatically. Delete operations still require approval. Dangerous commands are refused. |

AI-run commands are recorded in the application's audit/run history. The rc.11 client also includes fallback parsing for models that do not return native function calls and automatically retries without tools when a provider rejects function-calling requests.

In the AI input box, **Enter sends** and **Shift+Enter inserts a newline**.

## MCP / Agent integration

`kaduox-ssh-mcp.exe` is a local **stdio** MCP server. Example configuration:

```json
{
  "mcpServers": {
    "kaduox": {
      "command": "C:/Tools/Kaduox/kaduox-ssh-mcp.exe",
      "env": {
        "KADUOX_MCP_ALLOW_EXEC": "0",
        "KADUOX_MCP_ALLOW_MUTATIONS": "0"
      }
    }
  }
}
```

Replace the example path with the actual downloaded executable path. The Agent starts the process directly; no local HTTP service or separate terminal window is required.

Read-only host, route, basic-information, metric, and SFTP-listing tools are available by default. Remote execution and transfer/mutation capabilities require explicit permission switches. See [docs/MCP.md](docs/MCP.md) for the complete tool and permission contract.

## Command-line tools

| Program | Purpose |
| --- | --- |
| `kssh` | Single-target shell, command execution, SFTP transfer/sync, forwarding, inspection, credentials, host library, and diagnostics. |
| `kssh-tui` | Multi-host terminal dashboard with reusable authenticated sessions, SFTP workspace, shell, transfers, and bounded broadcast. |
| `kssh-fleet` | Bounded-concurrency command execution across explicit targets or inventory groups. |
| `kssh-inventory` | Offline inventory validation, host/group listing, and deterministic nested-group expansion. |
| `kssh-daemon` | Per-user local IPC daemon used by the source workspace for authenticated connection reuse. |
| `kaduox-ssh-mcp` | Local stdio MCP interface for Agents; read-only by default. |

All frontends reuse the shared Rust SSH core for authentication, host-key policy, ProxyJump/ProxyCommand, channels, SFTP, privilege switching, and transport limits.

## Core capabilities

The shared Rust core includes:

- SSH command execution and interactive PTY shells;
- OpenSSH `~/.ssh/config` resolution with bounded `Include` handling;
- ProxyJump and ProxyCommand transports;
- password, keyboard-interactive, Ed25519/ECDSA key, OpenSSH Agent, and Windows/Pageant-style agent authentication paths;
- local, remote, and SOCKS5 forwarding;
- strict / accept-new / explicit-insecure host-key verification;
- OpenSSH Host Certificate and revocation checks;
- bounded-memory SFTP upload/download and recursive transfers;
- resumable transfers, atomic destination staging, progress events, and cancellation;
- SFTP directory synchronization with a non-mutating plan before apply;
- explicit `--delete` and symbolic-link policies for destructive or recursive operations;
- connection reuse through explicit authenticated leases;
- fleet execution with bounded concurrency and per-target failure isolation.

For RSA-specific compatibility and the current fail-closed security policy, see [SECURITY.md](SECURITY.md) and the detailed documentation.

## Documentation

- [Desktop user guide (中文)](docs/DESKTOP_GUIDE.zh-CN.md)
- [MCP / Agent integration](docs/MCP.md)
- [TUI](docs/TUI.md)
- [Session workspace](docs/SESSION_WORKSPACE.md)
- [Fleet execution](docs/FLEET_EXEC.md)
- [Inventory](docs/INVENTORY.md)
- [OpenSSH config compatibility](docs/OPENSSH_CONFIG.md)
- [Host Certificates](docs/HOST_CERTIFICATES.md)
- [Security policy](SECURITY.md)
- [Release policy](docs/RELEASE.md)
- [Changelog](CHANGELOG.md)

## Build and test

The workspace is Rust-first and commits `Cargo.lock`; normal builds should use `--locked` where applicable. The repository contains Linux/macOS/Windows CI configuration, MSRV checks, Clippy, audit, release-policy checks, real-OpenSSH fixtures, performance benchmarks, SBOM generation, and stable-release signing policies.

The manually published **v0.33.0-rc.11** Windows preview does not claim that every cross-platform release gate was executed for those uploaded binaries. See the release notes for the validation performed for this preview.

## License

See [LICENSE](LICENSE).
