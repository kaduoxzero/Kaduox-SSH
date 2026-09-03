# Architecture

## Goals

Kaduox-SSH optimizes for four properties in this order:

1. Correctness and SSH security invariants.
2. Stable module boundaries that can support multiple frontends.
3. Predictable memory use and asynchronous concurrency.
4. Throughput/latency optimization backed by benchmarks rather than guesses.

## Core boundaries

`kaduox-ssh-core` owns protocol-facing behavior: connection setup, host-key verification, authentication, channels, command execution, PTY shells, privilege escalation, and SFTP.

Frontends own interaction policy: prompting for passwords/passphrases, raw-terminal mode, progress presentation, profile selection, key bindings, and UI state.

No frontend should reimplement SSH authentication or SFTP protocol logic.

## Session model

One authenticated SSH transport can host multiple independent channels. Kaduox-SSH keeps the transport alive and opens channels for shells, commands, SFTP, and later forwarding. This avoids unnecessary key exchange/authentication round trips and is the basis for fast user-context switching.

SSH does not provide a protocol operation that mutates the authenticated username after login. User-context switching is a remote OS operation:

- Interactive: allocate PTY, execute `sudo -iu <target>`.
- Non-interactive: execute `sudo -n -u <target> -- sh -lc <command>`.

The non-interactive path deliberately uses `-n`; a command must not hang waiting for an invisible sudo password prompt.

SFTP itself cannot inherit a `sudo` user context. A later privileged-transfer feature should upload into a writable staging location and then perform a deliberate remote `sudo install/mv` operation instead of pretending SFTP has changed uid.

## Connection-manager reuse contract

`ConnectionManager` may reuse an already authenticated transport, but a logical connection name is not sufficient proof that two connect requests are equivalent.

`ConnectionManager::connect(name, config, authentication)` only reuses an existing `name` when:

- the complete `ConnectionConfig` is equal, including resolved host/user/port, host-key policy and known-hosts path, ProxyCommand/ProxyJump route, identity-file configuration, keepalive/timeout behavior, and agent-forwarding policy; and
- the authentication source is compatible without retaining secret material.

Unencrypted private-key requests are keyed by configured key path, agent requests by the agent authentication source, and auto-authentication without a passphrase by its ordered configured identity sources. Any request that supplies a new secret — password, keyboard-interactive response, private-key passphrase, or Auto passphrase — is deliberately not eligible for implicit reuse. The new secret must never be silently ignored because an older authenticated transport already exists.

The same checks apply after concurrent connection races. If another task binds the same name while a candidate transport is authenticating, the losing candidate is closed and the winner is returned only when the configuration and authentication source are compatible.

`ConnectionManager::get(name)` is intentionally different: it explicitly leases whatever transport is currently bound to that logical name and performs no new compatibility check because no new connection request is supplied. Frontends should use `connect` for connect/reconnect intents and reserve `get` for deliberate access to an already selected session.

Changing host-key policy, route, login user, authentication source, or any other connection semantics requires removing the existing binding before reconnecting under the same logical name.

## Host-key policy

The default CLI policy is `accept-new`:

- known key: accept;
- unknown key: append to the standard OpenSSH `known_hosts` file;
- changed key: reject;
- bypass verification: only through explicit `insecure` policy.

Host certificates and `@cert-authority` policy are a future extension and must not silently weaken host verification.

## Authentication

The core currently supports password, private key, and Unix OpenSSH-agent authentication. Secrets are supplied for one connection and are not persisted by the core.

Agent authentication asks the local agent to sign the SSH authentication exchange; private-key material remains in the agent. Agent forwarding is a separate feature and will be opt-in when added.

## SFTP performance

The first invariant is bounded memory. Upload/download paths stream through a fixed 255 KiB buffer rather than reading complete files into memory.

Planned optimization work:

- query server SFTP limits;
- cap all server-advertised sizes before allocation;
- pipeline a bounded number of independent read/write requests;
- use adaptive concurrency based on RTT/window behavior;
- preserve ordering with a bounded reorder queue;
- benchmark loopback, WAN-emulated high-BDP links, many-small-file workloads, and multi-GB files;
- keep cancellation and handle shutdown correct under partial failure.

## Future crates

Split new crates only when the boundary has real independent value. Likely candidates are:

- `kaduox-ssh-config` for OpenSSH config/profile resolution;
- `kaduox-ssh-terminal` for terminal state/event processing;
- `kaduox-ssh-ui-*` frontends.

Avoid a large workspace of tiny crates before those boundaries stabilize.
