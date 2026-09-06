# Kaduox-SSH release process

Kaduox-SSH uses a fail-closed release path. A Git tag is not sufficient by itself to publish binaries: repository metadata, tag/version identity, branch reachability, validation policy, all target builds, native signing, target-specific SBOM generation, provenance policy, archive contents, and the final release asset set must pass their gates first.

## Version preparation

Both workspace crates inherit `[workspace.package].version`. Cargo also records the local package versions in `Cargo.lock`, so changing only `Cargo.toml` breaks the `--locked` contract.

Use the repository release tool instead of editing version strings independently:

```bash
python scripts/release/release_tool.py check
python scripts/release/release_tool.py set-version 0.27.0-rc.1
python scripts/release/release_tool.py check
```

`set-version` computes and validates all metadata edits before the first write. A version-only change should update the workspace version and both local `Cargo.lock` package versions without changing registry dependency versions or checksums.

## Required validation before tagging

A release candidate is not ready to tag until the required jobs actually execute successfully:

- Rust 1.98.1 formatting/check/test on Linux, macOS, and Windows;
- `cargo check --workspace --all-targets --locked`;
- `cargo test --workspace --locked`;
- Rust 1.85.0 MSRV check;
- Clippy with warnings denied on Rust 1.98.1;
- dependency audit with pinned `cargo-audit 0.22.0`;
- real OpenSSH integration fixtures;
- release packaging and published-asset qualification tests;
- target-specific SPDX generation/validation tests;
- immutable external Action pin tests;
- Rust toolchain, native-signing, attestation, and release-qualification policy tests;
- repository release metadata validation.

A GitHub Actions job that fails before runner allocation, checkout, or any workflow step execution is neither a pass nor evidence of a source-code failure. Promotion and tagging remain blocked in that state.

## Branch and tag boundary

Normal development is assembled on `integration/vX.Y.0-candidate` and promoted to `develop` only after its validation gates execute. Stable/major release work is promoted through the repository release process to `main`.

The release workflow accepts only an exact tag:

```text
v<workspace-version>
```

The tagged commit must be reachable from `main`; a tag placed directly on a feature, candidate, or unmerged develop commit is rejected. Maintainer tag-signing policy remains a repository/operator governance responsibility.

## Immutable workflow dependencies

Production release and release-readiness workflows use reviewed, immutable 40-character Git commit SHAs for all external Actions in:

- `.github/workflows/release.yml`;
- `.github/workflows/release-qualification.yml`;
- `.github/workflows/ci.yml`;
- `.github/workflows/quality.yml`;
- `.github/workflows/integration-openssh.yml`.

`scripts/release/test_action_pins.py` enforces the exact approved `action@sha` multiset and occurrence count for each workflow. Mutable tags/branches, abbreviated SHAs, unapproved Actions, missing human-readable version comments, unexpected occurrence counts, and unpinned container Actions fail closed.

Repository-local `./...` actions are tied to the checked-out repository commit and do not require an external Git ref.

## Pinned Rust toolchain policy

Normal CI, Clippy, dependency audit, real OpenSSH integration, and release builds use Rust **1.98.1** explicitly. Rust 1.98.1 replaced 1.98.0 after the Rust project issued the point release for the 1.98.0 vtable-generation miscompilation.

The workspace MSRV remains **Rust 1.85.0** and is tested separately. The audit job installs exactly `cargo-audit 0.22.0` using Rust 1.98.1. The RustSec advisory database is intentionally live security intelligence rather than a frozen build material.

`scripts/release/test_rust_toolchain_policy.py` rejects moving `stable`, changed protected toolchain versions, accidental MSRV changes, and restoration of moving audit toolchains.

## Release target matrix

The release matrix produces four binary suites:

- Linux x86_64: `x86_64-unknown-linux-gnu` (`.tar.gz`);
- macOS Intel: `x86_64-apple-darwin` (`.tar.gz`);
- macOS Apple Silicon: `aarch64-apple-darwin` (`.tar.gz`);
- Windows x86_64: `x86_64-pc-windows-msvc` (`.zip`).

Every suite contains all four frontends (`kssh`, `kssh-tui`, `kssh-fleet`, `kssh-inventory`), `README.md`, `README.zh-CN.md`, `LICENSE`, and `manifest.json`. The manifest records the target plus SHA-256 and byte length for every binary.

Archives are generated with deterministic metadata policy. Release packaging remains fail-closed on missing binaries/documents, changed binary declarations, unsafe path components, or oversized manifest output.

## Native signing and notarization

v0.25 makes native platform signing part of the stable release boundary.

For stable tags:

- Windows release binaries must be Authenticode-signed with the configured release certificate before packaging;
- macOS binaries must be Developer ID-signed and successfully notarized before packaging;
- the release path verifies the signed/notarized state according to the native-signing policy before publication.

Pre-release tags may opt into the same native-signing path through the existing repository configuration. Signing credentials remain external secrets and are never stored in the repository.

Native signing establishes platform publisher identity/trust-chain properties. SHA-256 checksums, SBOMs, and GitHub attestations complement native signing; they do not replace it.

## Target-specific SPDX SBOMs

v0.27 replaces the old single release-wide Cargo.lock inventory with one target-specific SPDX 2.3 JSON document per release target.

Each native build matrix job runs the pinned Cargo/Rust toolchain and executes:

```text
cargo metadata --format-version 1 --locked --filter-platform <target>
```

The target-filtered Cargo `resolve` graph is walked from `kaduox-ssh-cli`:

- development-only dependency edges are excluded;
- normal dependencies are represented as SPDX `DEPENDS_ON`;
- build dependencies remain part of the build-material graph using `BUILD_DEPENDENCY_OF`;
- every reachable metadata package must map to a committed `Cargo.lock` package identity so registry checksum data stays anchored to the lockfile;
- unknown dependency kinds, duplicate identities, missing package mappings, malformed checksums, or unexpectedly large metadata/SBOM output fail closed.

The four documents are named:

```text
kaduox-ssh-<version>-x86_64-unknown-linux-gnu.spdx.json
kaduox-ssh-<version>-x86_64-apple-darwin.spdx.json
kaduox-ssh-<version>-aarch64-apple-darwin.spdx.json
kaduox-ssh-<version>-x86_64-pc-windows-msvc.spdx.json
```

Each document namespace includes both release tag and target triple. The SPDX creation timestamp derives from the tagged source commit timestamp for repeatable generation.

The target SBOM is a Cargo dependency/build-material graph for that release target. Build dependencies are deliberately distinguished from runtime dependency edges; the document does not claim that every package is dynamically linked into every executable.

## Final release asset contract

Before GitHub Release creation, the publish job requires exactly:

- four release archives;
- four matching target-specific SPDX files;
- no unexpected files.

It creates `SHA256SUMS` over those eight primary assets and requires exactly eight checksum entries. The GitHub Release therefore contains the eight checksummed primary assets plus `SHA256SUMS`.

Pre-release SemVer tags containing `-` are created as GitHub pre-releases automatically.

## GitHub artifact attestation policy

v0.27 makes GitHub artifact attestations mandatory for **stable** tags.

The attestation job runs when either:

- the tag is stable (no pre-release suffix), or
- a pre-release explicitly enables `KADUOX_ENABLE_GITHUB_ATTESTATIONS=true`.

A stable publish job cannot accept an attestation result of `skipped`. If the stable attestation matrix fails or cannot run, stable publication is blocked rather than silently downgraded.

Each target receives its own attestation matrix job. That job downloads only `release-<target>` and requires exactly two inputs:

- that target's archive;
- that target's SPDX file.

The same reviewed `actions/attest` v4 commit is invoked twice per target:

1. provenance mode covers the archive and its SPDX asset together;
2. SBOM mode uses the archive as subject and the target SPDX file as `sbom-path`, explicitly binding the target SBOM predicate to the target archive.

The attestation job is restricted to `contents: read`, `id-token: write`, and `attestations: write`. It does not receive release-content write access. The OCI artifact storage-record option is not used for file attestations.

GitHub artifact attestations for private/internal repositories depend on GitHub account capability (currently GitHub Enterprise Cloud). The production stable channel treats unavailable attestation capability as a release blocker. Pre-releases may omit attestations unless explicitly enabled.

Consumers can verify a published archive with GitHub CLI when attestations are available, for example:

```bash
gh attestation verify ./kaduox-ssh-<version>-x86_64-unknown-linux-gnu.tar.gz \
  -R kaduoxzero/Kaduox-SSH
```

Attestation establishes an integrity/provenance statement about the workflow subject. It is not a vulnerability-free guarantee.

## Published release qualification

`.github/workflows/release-qualification.yml` runs on `release.published` and can also be dispatched manually for an exact tag. It downloads the **published bytes** and never recompiles a substitute artifact.

`scripts/release/qualification_tool.py` requires exactly the eight primary assets plus `SHA256SUMS`. It verifies all eight checksums and additionally validates each target SPDX document's:

- SPDX version/data license/document identity;
- exact release version and target-specific name;
- tag/target namespace;
- creation metadata declaring the target-filtered Cargo graph;
- Kaduox release root target marker;
- local Kaduox package presence;
- relationship references and document-to-root `DESCRIBES` relationship.

This content-level policy means replacing a Linux SBOM with a Windows-target document and recomputing `SHA256SUMS` still fails qualification.

For each platform archive, qualification validates archive paths/member types/modes/size budgets before extraction, verifies the manifest tag/version/target and binary sizes/digests, then launches all four packaged frontends with `--version`.

On Linux, the qualification workflow additionally runs the real OpenSSH integration suite against the extracted published `kssh` binary.

Post-publication qualification is an acceptance/announcement gate rather than a transactional rollback mechanism. A failed qualification requires correction or withdrawal before announcing the release.

## Verification after publication

Before announcing a release:

1. require the four-target Release Qualification workflow for the exact tag to execute successfully;
2. confirm it downloaded exactly four archives, four target SPDX files, and `SHA256SUMS`;
3. confirm all eight primary checksums were verified;
4. confirm each target's SPDX identity/namespace/target policy passed;
5. confirm each platform archive was safely extracted and all four packaged binaries returned the expected version;
6. require the Linux real OpenSSH suite against the published `kssh` to pass;
7. for stable releases, require the four target attestation jobs to succeed and verify archive attestations with `gh attestation verify`;
8. verify native Authenticode / Developer ID / notarization state according to the stable signing policy.

A job with no assigned runner or executed steps is not qualification evidence.

## V1 release-security boundary

The V1 release-security path now combines:

- deterministic archive/package manifests and SHA-256 checksums;
- immutable workflow Action pins;
- fixed production Rust toolchain plus separate MSRV validation;
- post-publication qualification of downloaded release bytes;
- Windows Authenticode signing;
- macOS Developer ID signing and notarization;
- target-specific SPDX dependency/build-material graphs;
- stable-release GitHub provenance and target-SBOM attestations.

Remaining V1 blockers should therefore be treated as **validation/operational readiness**, not missing release-policy design: the configured jobs must acquire runners and execute successfully, production signing/attestation account capabilities and credentials must be available, and the final V1 candidate must complete the full promotion/qualification path without bypassing these gates.
