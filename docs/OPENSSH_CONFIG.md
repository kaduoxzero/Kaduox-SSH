# OpenSSH client configuration compatibility

Kaduox-SSH resolves `~/.ssh/config` before opening a transport. Configuration
resolution is a security boundary: when Kaduox-SSH cannot preserve the relevant
OpenSSH meaning, it returns an error instead of silently falling back to a
direct/default connection.

## Host-scoped settings

The current parser resolves the supported `russh-config` host settings used by
Kaduox-SSH, including `HostName`, `User`, `Port`, `IdentityFile`,
`UserKnownHostsFile`, `ProxyCommand`, `ProxyJump`, and
`StrictHostKeyChecking`'s boolean representation.

OpenSSH's first-obtained-value behavior is preserved by processing configuration
in lexical order. The connection resolver is target-specific: it evaluates
structural `Host` / supported `Match` state for the original lookup alias and
flattens only effective options into an internal `Host *` stream before handing
the result to `russh-config`.

## Include support

v0.14 introduced a bounded, explicitly defined subset of OpenSSH `Include`.
v0.28 moves connection resolution to a host-aware scope machine so Include can
be combined with the supported Match subset without allowing an included file
to escape the caller's inactive scope. v0.29 adds bounded `${ENV}` expansion at
the shared Include-path anchoring boundary used by both connection resolution
and host-catalog discovery. v0.30 adds OpenSSH's context-free `%%` escape for a
literal percent character while leaving every named percent token fail-closed.

Supported behavior:

- `Include` may appear in the global scope or inside a `Host` or supported
  `Match` block;
- multiple include path arguments on one line;
- single-quoted and double-quoted paths plus backslash escaping;
- `${ENVIRONMENT_VARIABLE}` expansion from the client process environment;
- `%%` percent escaping, producing one literal `%` in the resulting path;
- absolute paths;
- relative paths anchored at `~/.ssh`, matching user-config OpenSSH behavior;
- `~/...` expansion for the current user's selected configuration home;
- `*` and `?` pathname wildcards;
- wildcard matches processed in deterministic lexical order;
- unmatched wildcard/literal paths are ignored;
- a leading `.` in a filename must be matched by an explicit leading `.` in the
  pattern, so `Include conf.d/*` cannot unexpectedly load `conf.d/.hidden`;
- nested includes;
- restoration of the containing file's active Host/Match state after every
  included file;
- an Include reached from an inactive Host/Match scope is still opened,
  trust-checked, cycle/budget checked, and parsed for supported structural and
  ordinary option syntax, but Host/Match declarations inside it cannot
  reactivate options for the current target;
- host-catalog discovery across the same bounded include graph, so aliases in
  included files appear in the TUI/OpenSSH host picker.

`${ENV}` and percent handling are deliberately single-pass, matching OpenSSH's
combined percent/dollar scanner. Environment lookup bytes are appended verbatim
and are not rescanned as nested `${...}` expressions or percent tokens. Source
`%%` collapses to one literal `%`; a plain `$` not followed by `{` remains a
literal character. Empty or unterminated `${...}` expressions, missing
variables, non-UTF-8 values, incomplete `%`, named percent tokens, and paths
whose final UTF-8 representation exceeds the 16 KiB Include path limit fail
closed. Expanded values are still checked for control characters and all
remaining unsupported path forms before filesystem access.

The inactive-scope rule is security-sensitive. OpenSSH parses an included file
with a never-match flag when the caller is inactive. v0.28 mirrors that property
instead of simply inlining the child's Host blocks, which could otherwise allow
a child `Host <target>` to become active even though the containing parent block
did not match.

The implementation intentionally has no shell invocation and performs no
command substitution.

## Supported Match subset

v0.19 introduced a deliberately narrow Match evaluator for trusted user config.
v0.28 integrates that same subset with Include; it does **not** broaden the set
of accepted Match criteria.

Supported forms are:

```text
Match all
Match originalhost <pattern-list>
```

`originalhost` is evaluated against the lookup alias before `HostName` rewriting.
Its pattern list is ASCII case-insensitive, comma-separated, supports `*` and
`?`, and honors leading `!` negated subpatterns. Evaluation is bounded by the
existing Match argument and pattern-size limits.

A matching block participates in normal first-obtained-value ordering. A
non-matching block is parse-only for this target. Includes nested under either
state retain the caller's active/never-match status and the parent state is
restored after the included file returns.

The following Match semantics remain fail-closed rather than approximated:

- `canonical` / `final` passes;
- `exec`;
- `localnetwork`;
- `host`;
- `tagged`;
- `command`;
- `user` / `localuser`;
- `version`;
- combined criteria;
- criterion negation and `criterion=value` forms;
- quoted or backslash-escaped Match arguments.

These require additional runtime/configuration context or parsing semantics and
must not be treated as aliases for the supported `originalhost` subset.

## User-config trust checks

v0.15 applies the same trust check at the single config-file read boundary, so
it covers the root `~/.ssh/config`, every nested `Include`, and the read-only
host-catalog path. v0.26 extends that boundary with native Windows owner/DACL
validation instead of treating regular-file validation as sufficient.

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

On Windows targets, v0.26 follows the Win32-OpenSSH user-config trust model at
the same read boundary:

- the resolved path must be a regular file;
- owner and DACL are queried from the already-open file `HANDLE` with
  `GetSecurityInfo`, so ACL validation and the bytes subsequently parsed refer
  to the same opened object instead of a second pathname lookup;
- the owner SID must be the current user, `BUILTIN\\Administrators`,
  `LocalSystem`, or `NT SERVICE\\TrustedInstaller` when that service SID can be
  resolved on the machine;
- a missing/NULL/invalid DACL is rejected; in particular, a NULL DACL is not
  confused with an empty DACL because NULL grants full access;
- ordinary `ACCESS_ALLOWED_ACE` entries for the trusted SIDs above are allowed;
- an otherwise-untrusted principal may retain read-only access, matching the
  `read_ok=1` policy Win32-OpenSSH applies to user configuration;
- an otherwise-untrusted principal is rejected if its allow ACE contains any of
  `FILE_WRITE_DATA`, `FILE_APPEND_DATA`, `FILE_WRITE_EA`,
  `FILE_WRITE_ATTRIBUTES`, `DELETE`, `WRITE_DAC`, `WRITE_OWNER`,
  `GENERIC_WRITE`, or `GENERIC_ALL`;
- the variable-length SID in every standard allow ACE must fit completely
  inside that ACE before the SID is passed to Windows validation helpers;
- advanced allow ACE layouts (object/callback/compound allow ACEs) fail closed
  instead of being partially decoded and potentially underestimating write
  authority.

Win32-OpenSSH also contains a reverse-account-name compatibility exception for
SIDHistory entries that resolve to the same account. Kaduox-SSH intentionally
does not broaden trust through account-name equivalence: distinct SIDs remain
distinct principals.

Configuration reads are bounded before allocation: one physical config file is
read through a 4 MiB + 1 byte probe and rejected if it exceeds 4 MiB. The Include
expansion layer separately enforces its 4 MiB cumulative input and expanded
output budgets.

## Include limits

Include expansion is bounded before parsing:

- maximum nesting depth: 16;
- maximum processed files: 256;
- maximum cumulative input/expanded configuration budget: 4 MiB;
- maximum physical config-file read: 4 MiB;
- maximum include arguments on one directive: 64;
- maximum final include path length after environment/`%%` expansion: 16 KiB;
- maximum wildcard path-component length: 1024 bytes.

Canonicalized active paths are tracked while recursing, so an include cycle
fails explicitly. Filesystem metadata, trust, permission, and read errors fail
rather than being converted into an empty include.

## Include forms that still fail closed

Current OpenSSH also supports forms that require additional parsing context or
full `glob(7)` behavior. Kaduox-SSH deliberately rejects them instead of
treating them as literal paths:

- named percent tokens (`%C`, `%L`, `%d`, `%h`, `%k`, `%l`, `%n`, `%p`, `%r`,
  `%u`, `%i`, `%j`); only context-free `%%` is supported;
- `~other-user/...` home expansion;
- bracket/collation wildcard expressions such as `[0-9]`, POSIX character
  classes, collating symbols, and equivalence classes.

`%d` is not approximated with Kaduox-SSH's selected config home: OpenSSH derives
`%d` from the local passwd entry's `pw_dir`, while Kaduox-SSH currently locates
its config home from platform environment variables. These values can differ,
so supporting `%d` requires an explicit local-user identity source rather than a
silent substitution.

These forms can be added only when their OpenSSH behavior is reproduced and
covered by fixtures on the supported platforms.

## Host-key and host-certificate trust boundary

v0.16 adds a separate fail-closed policy layer for `known_hosts`
`@cert-authority` and `@revoked` markers while preserving Russh's existing
ordinary unmarked host-key verification path.

Certificate algorithms are not advertised by default. Kaduox-SSH enables
certificate variants only when the effective `UserKnownHostsFile` contains a
matching `@cert-authority` for the specific target and host-key policy is not
`insecure`. This decision is made separately for the final target and every
ProxyJump hop. RSA host-key algorithms remain excluded by the existing hardening
policy, so RSA host-certificate variants are not advertised either.

A presented certificate is accepted only if it is a Host certificate, is signed
by one of the target's matching authorities, has a valid signature and current
validity interval, has no unsupported critical options, and either has no
principal restriction or contains a hostname principal matching the target.
Certificate principals use the hostname itself, not the non-default-port
`[host]:port` known-hosts representation. `*` and `?` principal wildcards are
supported.

Applicable `@revoked` entries reject an ordinary host key before normal
known-hosts or explicit-insecure acceptance. For certificates, both the
certified subject key and the signing CA are checked for revocation. A
certificate validation failure is never downgraded to ordinary embedded-key
verification.

Marker host patterns support comma-separated clear-text patterns, `*`, `?`, `!`
negation, ASCII case-insensitive clear-text matching, and OpenSSH `[host]:port`
formatting for non-default ports. OpenSSH hashed marker host names
(`|1|base64-salt|base64-hmac-sha1`) are also supported: the exact effective
known-hosts target bytes are checked with HMAC-SHA1, including the bracketed
non-default-port representation. Hashed names are therefore exact hashes rather
than case-folded wildcard patterns. Reads are bounded to 8 MiB.

See `HOST_CERTIFICATES.md` for the detailed certificate contract and current
limits.

## Validation boundary

Unit coverage includes Include ordering, global/Host scope restoration, inactive
Host/Match Include non-reactivation, supported Match+Include resolution,
`${ENV}` single-pass expansion and failure/resource bounds, OpenSSH `%%`
literal-percent collapse, named/incomplete percent-token rejection, inactive
ordinary-option syntax validation, unsupported Match rejection, nested cycles,
hidden-file wildcard behavior, unsupported path-form rejection, catalog
discovery, regular-file enforcement, accepted Unix 0600/0640/0644 modes,
rejected group/other-writable modes, an insecure nested Include fixture,
oversized single-file rejection, Windows read-only/untrusted-write ACL cases,
clear and hashed marker host-pattern matching, CA/revocation classification, and
certificate-principal matching.

The real OpenSSH workflow also contains a host-certificate fixture that creates
an Ed25519 CA and `HostCertificate` with `ssh-keygen`/`sshd`, then tests
trusted-CA success, principal mismatch, revoked CA rejection, and ordinary
host-key revocation overriding explicit insecure policy.

Candidate promotion still requires the repository's real CI, Clippy, audit,
release-policy, and OpenSSH jobs to acquire runners and execute; a workflow that
ends with no steps executed is not considered validation.
