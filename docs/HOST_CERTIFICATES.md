# OpenSSH host certificate trust

Kaduox-SSH v0.16 adds fail-closed OpenSSH host-certificate verification on top of the existing ordinary `known_hosts` host-key path.

## When certificate algorithms are advertised

Host-certificate algorithms are disabled by default. Before a handshake, Kaduox-SSH reads the effective `UserKnownHostsFile` for the specific target. Certificate variants are advertised only when that target has at least one matching clear-text **Ed25519 or ECDSA** `@cert-authority` entry and host-key verification is not `insecure`.

The decision is made independently for the final target and for every ProxyJump hop. A CA trusted for one host therefore does not enable certificate negotiation for another hop.

Kaduox-SSH preserves its existing hardened host-key algorithm policy. RSA host keys are currently excluded, so RSA host-certificate variants are also not advertised. A CA key's algorithm does not need to match the certified host key's algorithm, but v0.16 deliberately limits trusted certificate-signature verification to Ed25519/ECDSA CA keys while Russh's optional RSA feature remains disabled.

## Certificate verification

A presented certificate is accepted only when all of the following hold:

- it is an OpenSSH **host** certificate, not a user certificate;
- its signing key is an enabled Ed25519/ECDSA verifier algorithm and matches a `@cert-authority` entry whose host pattern applies to the current target and port;
- the certificate signature verifies;
- the current time is inside the certificate validity interval;
- neither the certified host public key nor its signing CA is matched by an applicable `@revoked` entry;
- the certificate has no critical options that Kaduox-SSH does not understand;
- when principals are present, at least one principal matches the target hostname. Host-certificate principals use the hostname itself, not the `[host]:port` representation used by `known_hosts` for non-default ports.

OpenSSH host certificate principals may contain `*` and `?`; Kaduox-SSH applies those wildcards case-insensitively to ASCII hostnames. An empty certificate principal list follows OpenSSH certificate semantics and does not add a hostname restriction.

Non-critical certificate extensions do not weaken host identity and are not treated as authorization to alter Kaduox-SSH behavior.

A certificate is never converted into its embedded public key and accepted through the ordinary known-hosts path. Certificate validation failure rejects the handshake.

## `@revoked`

For clear-text marker entries, revocation is checked before ordinary host-key acceptance. This applies even when the caller explicitly selects `--host-key insecure`: a matching `@revoked` ordinary host key remains rejected.

For host certificates, both the certified subject key and the signing CA key are checked. Either revocation rejects the certificate.

## Marker host patterns

v0.16 supports clear-text marker host patterns with:

- comma-separated pattern lists;
- `*` and `?` wildcards;
- `!` negation;
- ASCII case-insensitive matching;
- ordinary `host` representation on port 22;
- OpenSSH `[host]:port` representation on non-default ports.

The marker-policy reader is bounded to 8 MiB.

## Explicit fail-closed limits

The following are not claimed as compatible in v0.16:

- hashed `@cert-authority` or `@revoked` host patterns (`|1|...`): these cause an explicit error rather than silently dropping CA or revocation policy;
- RSA host certificates and RSA CA certificate-signature verification: the existing security policy keeps Russh's optional RSA feature disabled;
- certificate-form keys inside `@revoked` entries: v0.16's marker layer accepts public-key entries and fails closed on unsupported marker key syntax;
- certificate critical options: no critical option is currently interpreted, so any critical option rejects the certificate;
- full OpenSSH `Match` evaluation and the remaining user-config expansion forms documented in `OPENSSH_CONFIG.md`;
- Windows ACL-equivalent trust validation for user configuration files.

Ordinary unmarked hashed `known_hosts` entries continue to use Russh's existing ordinary host-key verification path; the hashed-marker restriction is specifically about the new CA/revocation policy layer.

## Validation

The real OpenSSH integration workflow contains a dedicated host-certificate fixture. It generates an Ed25519 CA and host key with `ssh-keygen`, starts an `sshd` using `HostCertificate`, and exercises:

- successful connection through a matching `@cert-authority`;
- rejection on hostname-principal mismatch;
- rejection when the signing CA is marked `@revoked`;
- rejection of an ordinary `@revoked` host key even under explicit insecure policy.

As with the rest of the pre-1.0 candidate line, these fixtures are not considered passed until GitHub Actions jobs actually acquire runners and execute their steps.
