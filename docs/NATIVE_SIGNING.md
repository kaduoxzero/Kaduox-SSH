# Native release signing

Kaduox-SSH v0.25 adds native platform signing to the production tag-release path without storing private signing material in the repository.

## Release policy

A SemVer tag without a prerelease suffix, such as `v1.0.0`, is a stable release. Stable releases always execute the Windows and macOS signing steps. Missing, malformed, expired, untrusted, or rejected signing credentials therefore fail the corresponding build job and block publication.

A prerelease tag containing `-`, such as `v1.0.0-rc.1`, remains usable for unsigned release-pipeline qualification. To exercise native signing on a prerelease, set the repository variable:

```text
KADUOX_ENABLE_NATIVE_SIGNING=true
```

This variable never disables native signing for a stable tag.

Signing occurs after the four binaries are built but before `--version` smoke tests and deterministic packaging. `manifest.json` and release archive checksums therefore describe the final signed executable bytes.

## Windows Authenticode

Required GitHub Secrets:

```text
KADUOX_WINDOWS_SIGNING_PFX_BASE64
KADUOX_WINDOWS_SIGNING_PFX_PASSWORD
```

Required repository variable:

```text
KADUOX_WINDOWS_TIMESTAMP_URL
```

The PFX must contain a trusted code-signing certificate and private key. The workflow decodes it only into the GitHub-hosted runner temporary directory. `scripts/release/sign-windows.ps1` signs all four `.exe` files with SHA-256 and an RFC 3161 timestamp, then runs SignTool verification with the Default Authentication policy and requires a timestamp. The temporary PFX is removed in a `finally` block.

The timestamp URL is deliberately an operator-controlled variable rather than a hard-coded third-party service. It must identify an RFC 3161 timestamp service compatible with the certificate policy.

## macOS Developer ID and notarization

Required GitHub Secrets:

```text
KADUOX_MACOS_SIGNING_CERT_P12_BASE64
KADUOX_MACOS_SIGNING_CERT_PASSWORD
KADUOX_APPLE_NOTARY_KEY_P8_BASE64
```

Required repository variables:

```text
KADUOX_MACOS_SIGNING_IDENTITY
KADUOX_APPLE_NOTARY_KEY_ID
KADUOX_APPLE_NOTARY_ISSUER_ID
```

The P12 must contain a valid Developer ID Application identity appropriate for signing command-line tools. The App Store Connect API private key is used only for `notarytool`; the workflow does not use an Apple ID or app-specific password.

`scripts/release/sign-macos.sh` creates an isolated temporary keychain, imports the Developer ID identity, signs all four binaries with a secure timestamp and hardened runtime, and verifies each signature. It then creates a temporary ZIP containing the signed binaries and submits that ZIP with `xcrun notarytool submit --wait`. Publication is blocked unless the returned notarization status is exactly `Accepted`.

The temporary P12, API key, ZIP, result JSON, and keychain are removed by an EXIT cleanup handler.

Kaduox-SSH currently distributes macOS release suites as `.tar.gz`. A tar archive is not a staplable notarization container, so v0.25 does not claim to staple a ticket to the tarball. The notarization service records acceptance for the signed code submitted inside the temporary ZIP. Post-publication qualification uses `codesign` and `spctl --assess --type exec` against the extracted published binaries so the release gate verifies the bytes users actually download.

## Post-publication verification

For stable releases, and for prereleases when native signing was enabled, `.github/workflows/release-qualification.yml` performs signature checks without access to private signing credentials:

- Windows: `signtool verify /pa /v /tw` for all four published executables;
- macOS: strict `codesign` verification, secure timestamp presence, and `spctl --assess --type exec` for all four published executables.

Linux currently has no equivalent repository-managed native publisher signature. Its published archive remains covered by the release checksum/manifest/provenance controls.

## Credential handling rules

- Never commit PFX, P12, P8, passwords, or base64-encoded private material.
- Store private material only in GitHub Secrets or a future external signing service integration.
- Keep certificate identities, key IDs, issuer IDs, and timestamp URLs in repository variables only when they are non-secret metadata.
- Rotate signing material according to certificate/provider policy and test rotation on a prerelease before the next stable release.
- Do not weaken the stable-release gate to work around an expired or unavailable signing credential. Fix or rotate the credential instead.

The GitHub-hosted runner allocation incident currently prevents these signing paths from being exercised in this repository. Their presence in source is not evidence of a successful Authenticode or Apple notarization ceremony; V1 release qualification still requires an actual signed release run.
