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
- Kaduox-SSH must not perform RSA private-key operations through Russh's optional RustCrypto RSA signer while `RUSTSEC-2023-0071` remains unresolved. The `russh/rsa` feature is disabled and the affected `rsa` crate must remain absent from `Cargo.lock`.
- Direct local RSA private-key authentication therefore fails closed with a remediation message. RSA user authentication may use an external SSH agent because the private signing operation remains inside the agent.
- RSA server host keys remain an allowed compatibility path because host-key verification is a public-key operation and does not require Kaduox-SSH to possess or operate on an RSA private exponent.

## RSA compatibility boundary

The RSA restriction is deliberately narrow rather than a blanket ban. Ed25519/ECDSA direct private-key authentication is supported. RSA identities supplied by an external OpenSSH agent or Pageant-compatible agent can still be used, subject to the agent implementation's own security posture. Kaduox-SSH negotiates the appropriate signature hash and forwards the signing request; it does not import the RSA private key into the process.

The OpenSSH integration suite includes three independent regression checks: direct RSA private-key files are rejected, the same RSA identity succeeds through an external agent, and an RSA-host-key-only SSH server can still be authenticated with a non-RSA client key. Any change to crypto features must keep all three behaviors explicit.

## Dependency and CI policy

The application commits `Cargo.lock`; normal CI builds with `--locked`. The quality workflow runs Clippy with warnings denied and scans the resolved dependency graph with the RustSec audit action. Real OpenSSH integration tests exercise security-sensitive protocol paths against disposable `sshd` instances.

Dependency feature changes that alter the resolved graph must update `Cargo.lock` in the same development change. Security advisories are not ignored merely to make CI green: an exception requires a documented threat analysis, bounded exposure, an upstream tracking reference, and a concrete removal condition.
