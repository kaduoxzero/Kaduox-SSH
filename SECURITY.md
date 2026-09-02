# Security Policy

Kaduox-SSH handles authentication material, host identity, remote command execution, privilege escalation, agent forwarding, and file synchronization. Treat security changes as protocol-boundary changes rather than ordinary UI changes.

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's private vulnerability reporting/security-advisory flow when it is enabled for this repository. Do not publish credentials, private keys, host keys, agent data, or exploit details in a public issue.

A useful report includes the affected version/commit, operating system, SSH server implementation/version, authentication method, exact security impact, and a minimal reproduction that does not contain real secrets.

## Supported development line

Security fixes are developed and validated on `develop` first. Stable fixes are promoted to `main` through the release process after the normal validation gates pass.

## Security invariants

- Host-key changes must fail closed unless the caller explicitly selects the insecure test policy.
- Agent forwarding is opt-in and must never copy private-key material to the remote host.
- Privileged uploads stage as the authenticated user and perform an explicit non-interactive sudo install/move; SFTP itself is never represented as changing uid.
- Synchronization preserves remote-only entries by default. Destructive removal requires explicit `--delete`.
- Recursive transfer/sync does not follow symbolic links until an explicit, tested policy exists.
- Passwords and private-key passphrases must not be accepted as ordinary command-line string arguments where they would be exposed through shell history or process inspection.
- Full OpenSSH host-certificate / `@cert-authority` support must not be advertised until certificate signatures, principals, critical options, validity, host patterns, and revocation behavior are all validated.

## Dependency and CI policy

The application commits `Cargo.lock`; normal CI builds with `--locked`. The quality workflow runs Clippy with warnings denied and scans the resolved dependency graph with `cargo audit`. Real OpenSSH integration tests exercise security-sensitive protocol paths against disposable `sshd` instances.
