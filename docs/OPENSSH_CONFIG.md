# OpenSSH client configuration compatibility

Kaduox-SSH resolves `~/.ssh/config` before opening a transport. Configuration
resolution is a security boundary: when Kaduox-SSH cannot preserve the
relevant OpenSSH meaning, it returns an error instead of silently falling back
to a direct/default connection.

## Host-scoped settings

The current parser resolves the supported `russh-config` host settings used by
Kaduox-SSH, including `HostName`, `User`, `Port`, `IdentityFile`,
`UserKnownHostsFile`, `ProxyCommand`, `ProxyJump`, and
`StrictHostKeyChecking`'s boolean representation.

OpenSSH's first-obtained-value behavior is preserved by keeping configuration
order intact. Kaduox-SSH also represents the implicit global scope before the
first `Host` as an internal `Host *` scope so ordinary top-level defaults are
not discarded by the downstream host-only parser.

## Include support

v0.14 expands a bounded, explicitly defined subset of OpenSSH `Include`
before handing configuration to `russh-config`.

Supported behavior:

- `Include` may appear in the global scope or inside a `Host` block;
- multiple include path arguments on one line;
- single-quoted and double-quoted paths plus backslash escaping;
- absolute paths;
- relative paths anchored at `~/.ssh`, matching user-config OpenSSH behavior;
- `~/...` expansion for the current user's home directory;
- `*` and `?` pathname wildcards;
- wildcard matches processed in deterministic lexical order;
- unmatched wildcard/literal paths are ignored;
- a leading `.` in a filename must be matched by an explicit leading `.` in
  the pattern, so `Include conf.d/*` cannot unexpectedly load
  `conf.d/.hidden`;
- nested includes;
- restoration of the containing global/`Host` scope after every included
  file, so a `Host` block inside an included file cannot capture declarations
  that follow the `Include` in its parent file;
- host-catalog discovery across the same bounded include graph, so aliases in
  included files appear in the TUI/OpenSSH host picker.

The implementation intentionally has no shell invocation and performs no
command substitution.

## User-config trust checks

v0.15 applies the same trust check at the single config-file read boundary, so
it covers the root `~/.ssh/config`, every nested `Include`, and the read-only
host-catalog path.

On Unix targets:

- the resolved path must be a regular file;
- the file owner must be the process real uid returned by `getuid()` or uid 0
  (`root`);
- group and other write bits must both be clear (`mode & 0022 == 0`);
- each path is opened once, metadata is checked on that opened file descriptor,
  and configuration bytes are then read from the same descriptor; the trust
  decision and parsed bytes therefore cannot be redirected by replacing the
  pathname between a separate `stat` and `open`;
- symlink paths are evaluated through the file opened from that path, and the
  trust decision applies to the resolved target file metadata.

The implementation deliberately uses the process uid rather than trusting the
owner of a path selected through `$HOME`. Redirecting `$HOME` therefore cannot
change which uid is trusted to author SSH configuration.

Configuration reads are also bounded before allocation: one physical config
file is read through a 4 MiB + 1 byte probe and rejected if it exceeds 4 MiB.
The Include expansion layer separately enforces its 4 MiB cumulative input and
expanded-output budgets.

Windows does not use POSIX uid/mode semantics. Kaduox-SSH still requires the
resolved config path to be a regular file there, but v0.15 does **not** claim
that this is equivalent to OpenSSH's Windows ACL trust policy. A proper ACL
implementation remains separate V1 security work rather than translating Unix
mode checks into a misleading approximation.

## Include limits

Include expansion is bounded before parsing:

- maximum nesting depth: 16;
- maximum processed files: 256;
- maximum cumulative input/expanded configuration budget: 4 MiB;
- maximum physical config-file read: 4 MiB;
- maximum include arguments on one directive: 64;
- maximum include path length: 16 KiB;
- maximum wildcard path-component length: 1024 bytes.

Canonicalized active paths are tracked while recursing, so an include cycle
fails explicitly. Filesystem metadata, trust, permission, and read errors fail
rather than being converted into an empty include.

## Include forms that still fail closed

Current OpenSSH also supports forms that require additional parsing context or
full `glob(7)` behavior. Kaduox-SSH deliberately rejects them instead of
treating them as literal paths:

- `%` token expansion;
- `${ENVIRONMENT_VARIABLE}` expansion;
- `~other-user/...` home expansion;
- bracket/collation wildcard expressions such as `[0-9]`, POSIX character
  classes, collating symbols, and equivalence classes.

These can be added only when their OpenSSH behavior is reproduced and covered
by fixtures on the supported platforms.

## Match remains unsupported for connection resolution

`Match` is not equivalent to a second spelling of `Host`. Modern OpenSSH can
condition it on criteria such as host/original host, user/local user,
canonical/final pass, command/session context, `exec`, local network, tags,
and version. Evaluating only a subset can apply credentials, proxy routes, or
host-key settings to the wrong destination.

Therefore every `Match` encountered in the root config or any included file
causes connection resolution to fail before transport creation. The host
catalog is read-only: it may still list concrete `Host` aliases from the
expanded include graph while marking the catalog as containing unsupported
structural configuration.

## Host-key and host-certificate trust boundary

v0.16 adds a separate fail-closed policy layer for clear-text `known_hosts`
`@cert-authority` and `@revoked` markers while preserving Russh's existing
ordinary unmarked host-key verification path.

Certificate algorithms are not advertised by default. Kaduox-SSH enables
certificate variants only when the effective `UserKnownHostsFile` contains a
matching `@cert-authority` for the specific target and host-key policy is not
`insecure`. This decision is made separately for the final target and every
ProxyJump hop. RSA host-key algorithms remain excluded by the existing
hardening policy, so RSA host-certificate variants are not advertised either.

A presented certificate is accepted only if it is a Host certificate, is
signed by one of the target's matching authorities, has a valid signature and
current validity interval, has no unsupported critical options, and either has
no principal restriction or contains a hostname principal matching the target.
Certificate principals use the hostname itself, not the non-default-port
`[host]:port` known-hosts representation. `*` and `?` principal wildcards are
supported.

Applicable `@revoked` entries reject an ordinary host key before normal
known-hosts or explicit-insecure acceptance. For certificates, both the
certified subject key and the signing CA are checked for revocation. A
certificate validation failure is never downgraded to ordinary embedded-key
verification.

The v0.16 marker-policy reader supports comma-separated clear-text patterns,
`*`, `?`, `!` negation, ASCII case-insensitive matching, and OpenSSH
`[host]:port` formatting for non-default ports. Reads are bounded to 8 MiB.
Hashed marker patterns (`|1|...`) are intentionally rejected rather than
silently dropping CA/revocation policy. Ordinary unmarked hashed known-hosts
entries continue through Russh's existing ordinary host-key path.

See `HOST_CERTIFICATES.md` for the detailed certificate contract and current
limits.

## Validation boundary

Unit coverage includes include ordering, global/Host scope restoration, nested
cycles, hidden-file wildcard behavior, unsupported expansion rejection,
catalog discovery, regular-file enforcement, accepted Unix 0600/0640/0644
modes, rejected group/other-writable modes, an insecure nested Include fixture,
oversized single-file rejection, marker host-pattern matching, CA/revocation
classification, and certificate-principal matching.

The real OpenSSH workflow also contains a v0.16 host-certificate fixture that
creates an Ed25519 CA and `HostCertificate` with `ssh-keygen`/`sshd`, then tests
trusted-CA success, principal mismatch, revoked CA rejection, and ordinary
host-key revocation overriding explicit insecure policy.

Candidate promotion still requires the repository's real CI, Clippy, audit,
release-policy, and OpenSSH jobs to acquire runners and execute; a workflow
that ends with no steps executed is not considered validation.
