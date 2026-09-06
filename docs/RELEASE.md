# Kaduox-SSH release process

Kaduox-SSH uses a fail-closed release path. A Git tag is not sufficient by
itself to publish binaries: repository metadata, tag/version identity, branch
reachability, validation policy, all target builds, archive contents, the SBOM,
and the final release asset set must pass their gates first.

## Version preparation

Both workspace crates inherit `[workspace.package].version`. Cargo also records
the local package versions in `Cargo.lock`, so changing only `Cargo.toml` breaks
the `--locked` contract.

Use the repository release tool instead of editing the version strings by hand:

```bash
python scripts/release/release_tool.py check
python scripts/release/release_tool.py set-version 0.24.0-rc.1
python scripts/release/release_tool.py check
```

`set-version` validates the current repository before writing. It computes the
workspace and both lockfile edits before the first write and refuses unexpected
TOML/lock structure. Review the resulting diff before committing; a version-only
commit should change the workspace version and the two local Cargo.lock package
versions, not registry dependency versions or checksums.

## Required validation before tagging

A release candidate is not ready to tag until all of these execute successfully:

- Rust 1.98.1 formatting/check/test on the normal release-readiness lanes;
- `cargo check --workspace --all-targets --locked` on Linux, macOS, and Windows;
- `cargo test --workspace --locked`;
- Rust 1.85.0 MSRV check;
- Clippy with warnings denied on Rust 1.98.1;
- dependency audit using `cargo-audit 0.22.0` built and executed with Rust 1.98.1;
- real OpenSSH integration fixtures built with Rust 1.98.1;
- release packaging, published-asset qualification, and SPDX SBOM Python unit tests;
- immutable workflow-action pin policy tests;
- Rust toolchain and release-qualification workflow policy tests;
- repository release metadata validation.

A GitHub Actions job that fails before runner allocation, checkout, or any step
execution is neither a pass nor evidence of a source-code failure. Promotion and
tagging remain blocked in that state.

## Branch and tag boundary

Normal development is assembled on `integration/vX.Y.0-candidate` and promoted
to `develop` only after its validation gates execute. Major/final release work
is promoted through the repository's release process to `main`.

The release workflow accepts only an exact tag of the form:

```text
v<workspace-version>
```

For example, workspace version `1.0.0` requires tag `v1.0.0`. A tag mismatch
fails before compilation. The tagged commit must also be reachable from `main`;
a tag placed directly on a feature, candidate, or unmerged develop commit is
rejected.

Use an annotated/signed Git tag according to the maintainer signing policy. The
workflow verifies that the pushed tag exists, but tag signing policy remains an
operator/repository-governance responsibility.

## Immutable workflow dependencies

The production release workflow and workflows that decide or verify release
readiness must not execute mutable GitHub Action tags or branches. External
`uses:` entries in the following files are pinned to reviewed, full 40-character
Git commit SHAs:

- `.github/workflows/release.yml`;
- `.github/workflows/release-qualification.yml`;
- `.github/workflows/ci.yml`;
- `.github/workflows/quality.yml`;
- `.github/workflows/integration-openssh.yml`.

A human-readable version/ref comment remains beside each SHA so maintainers can
identify the intended upstream release. `scripts/release/test_action_pins.py`
maintains an exact expected multiset of `action@sha` references for each workflow,
including occurrence counts. This means the policy rejects not only mutable refs
such as `@v4` or `@stable`, abbreviated hashes, and unknown Actions, but also:

- a reviewed Action being moved to a different full SHA without a policy update;
- one occurrence being replaced by a different otherwise-approved SHA;
- adding or removing an Action occurrence without explicit review;
- removing the human-readable version/ref comment;
- unpinned `docker://` actions.

Repository-local `./...` actions are permitted because their contents are fixed
by the checked-out repository commit. Updating any external workflow dependency
therefore requires changing both the workflow reference and the exact expected
pin policy in the same reviewed change.

## Pinned Rust toolchain policy

v0.23 removes the moving Rust `stable` compiler channel from the workflows that
build, test, audit, and package Kaduox-SSH. Normal CI, Clippy, real OpenSSH
integration, dependency audit, and the four-platform release build use Rust
**1.98.1** explicitly. Rust 1.98.1 is selected instead of 1.98.0 because the Rust
project released 1.98.1 on 2026-09-03 to fix a vtable-generation miscompilation
in 1.98.0.

The MSRV lane remains a distinct Rust **1.85.0** check. The workspace's declared
minimum Rust version is therefore not silently raised to the release compiler.

Quality no longer delegates dependency auditing to a composite Action that
invokes `cargo +stable`. The audit job installs the same Rust 1.98.1 toolchain,
uses SHA-pinned `actions/cache` v5.0.1 for the scanner binary, installs exactly
`cargo-audit 0.22.0` with `--locked --no-default-features` on cache misses, and
runs `cargo +1.98.1 audit --file Cargo.lock`.

`scripts/release/test_rust_toolchain_policy.py` enforces the exact protected
workflow toolchain counts and fails closed on missing toolchain inputs,
`toolchain: stable`, `cargo +stable`, changes to the 1.98.1 release compiler,
changes to the 1.85.0 MSRV lane, or restoration of the moving composite audit
path. The release-qualification workflow intentionally has no Rust toolchain and
must not run `cargo build`: it verifies published binaries instead of rebuilding
them.

The RustSec advisory database is intentionally **not** frozen: it is live security
intelligence, not a build material. The reproducibility claim fixes the compiler,
scanner version, workflow Action commits, source, and Cargo.lock dependency graph;
it does not make the vulnerability database stale for the sake of byte identity.

## Release artifacts

The release matrix currently produces four binary suites:

- Linux x86_64: `x86_64-unknown-linux-gnu` (`.tar.gz`);
- macOS Intel: `x86_64-apple-darwin` (`.tar.gz`);
- macOS Apple Silicon: `aarch64-apple-darwin` (`.tar.gz`);
- Windows x86_64: `x86_64-pc-windows-msvc` (`.zip`).

Every suite must contain all four frontends:

- `kssh`;
- `kssh-tui`;
- `kssh-fleet`;
- `kssh-inventory`.

It also includes `README.md`, `README.zh-CN.md`, `LICENSE`, and `manifest.json`.
The manifest records the target plus SHA-256 and byte length for every binary.

v0.20 adds one release-wide SPDX 2.3 JSON asset named
`kaduox-ssh-<version>.spdx.json`. It is generated directly from the committed
Cargo.lock v4 graph and contains stable package identities plus `DEPENDS_ON`
relationships. Ambiguous dependency references, malformed package checksums,
duplicate package identities, or an unexpectedly large document fail closed.
The SPDX creation timestamp is derived from the tagged source commit so repeated
generation from the same release source remains reproducible.

The SPDX file is deliberately described as a **Cargo.lock dependency inventory**,
not as a platform-pruned binary bill of materials. Cargo.lock records the
workspace's resolved dependency universe and may contain target-specific packages
that are not linked into every Linux, macOS, or Windows executable. Kaduox-SSH
does not claim target-level package precision until the release process has a
validated build-material graph for each target.

The publish job refuses any release with anything other than exactly four final
archives and exactly one release-wide SPDX file. It creates a top-level
`SHA256SUMS` covering all five assets.

Pre-release SemVer tags containing `-` (for example `v0.24.0-rc.1`) are created
as GitHub pre-releases automatically.

## Optional GitHub artifact attestations

GitHub artifact attestations are an opt-in release capability. They are disabled
unless the repository variable below is set exactly to `true`:

```text
KADUOX_ENABLE_GITHUB_ATTESTATIONS=true
```

When enabled, a dedicated attestation job receives only the permissions required
for GitHub OIDC/Sigstore attestation (`contents: read`, `id-token: write`, and
`attestations: write`). Normal build and SBOM-generation jobs keep
`contents: read` only.

The attestation job downloads the complete release asset set, verifies that it
contains exactly four archives and exactly one SPDX JSON file with no unexpected
files, and then creates a build-provenance attestation covering all five release
assets. If attestation is enabled and this step fails, publication is blocked.
If the variable is not enabled, the attestation job is skipped and the normal
archive/SBOM release path remains available.

v0.20 intentionally does **not** create a GitHub SBOM predicate that binds the
release-wide Cargo.lock inventory to an individual platform archive. That would
claim a target-level correspondence the lockfile-only inventory cannot prove.
A future target-specific SBOM attestation must be backed by a validated
per-target build-material graph first.

GitHub currently permits artifact attestations for private/internal repositories
only on GitHub Enterprise Cloud. Kaduox-SSH therefore does not enable the feature
unconditionally for this private repository or pretend that repository settings
can substitute for the required GitHub account capability.

Consumers of an attested release can verify an archive or the SPDX asset with the
GitHub CLI, for example:

```bash
gh attestation verify ./kaduox-ssh-<version>-x86_64-unknown-linux-gnu.tar.gz \
  -R kaduoxzero/Kaduox-SSH
```

An attestation establishes provenance and an integrity relationship to the build
workflow; it is not a statement that the software is vulnerability-free.

## Published release qualification

v0.24 adds `.github/workflows/release-qualification.yml`. It runs on the
`release.published` event and can be dispatched manually for an exact release tag.
Each of the same four target platforms checks out qualification code from that tag,
downloads the assets attached to that GitHub Release, and validates the downloaded
bytes rather than any local build directory.

`scripts/release/qualification_tool.py` requires exactly four target archives,
one release-wide SPDX JSON file, and `SHA256SUMS`. It verifies all five SHA-256
entries, validates archive paths/member types/modes/size budgets before extraction,
checks the exact manifest tag/version/target/binary sizes and digests, then launches
all four packaged frontends with `--version` from the extracted archive.

On Linux, the qualification workflow additionally sets `KSSH` to the extracted
published `kssh` and runs the existing real OpenSSH integration suite. This makes
direct SSH execution, proxying, agent paths, SFTP upload/download, privileged
transfer, sync, and forwarding run against the binary users actually downloaded.
It does not compile a replacement binary during qualification.

Post-publication qualification is an acceptance/announcement gate, not a
transactional rollback mechanism: the GitHub Release already exists when the
`release.published` event runs. A failed qualification must therefore block
announcement/acceptance and require correction or withdrawal of the release.

## Verification after publication

Before announcing a release:

1. require the four-target `Release Qualification` workflow for the exact tag to
   execute successfully;
2. confirm it downloaded exactly four archives, the SPDX JSON file, and
   `SHA256SUMS` from the GitHub Release and verified all five checksums;
3. confirm each target safely extracted its expected archive, validated its
   `manifest.json`, and launched all four packaged frontends with `--version`;
4. require the Linux job's real OpenSSH suite to pass against the extracted
   published `kssh` binary;
5. inspect the SPDX document and confirm its tag/version namespace and local
   Kaduox package records;
6. when GitHub attestations are enabled, verify each archive and the SPDX asset
   with `gh attestation verify`.

The qualification workflow can be rerun manually for an existing release tag if
infrastructure prevented the automatic `release.published` run from executing.
A job with no assigned runner/steps is not qualification evidence.

## V1 signing boundary

SHA-256 checksums detect corruption and let users compare bytes against the
published checksum file, but they are **not** platform code signing and do not
by themselves establish publisher identity if the release account is
compromised.

v0.20 provides a deterministic release-wide SPDX dependency inventory and an
optional GitHub provenance-attestation path. v0.21 pins production release Action
implementations to reviewed immutable commits. v0.22 extends that immutable
Action boundary to CI, Quality, dependency audit, and real OpenSSH validation.
v0.23 fixes the normal build/test/audit/release compiler to Rust 1.98.1 while
preserving the Rust 1.85.0 MSRV lane. v0.24 adds cross-platform post-publication
qualification of the actual GitHub Release assets and a real Linux OpenSSH suite
against the extracted published binary. These controls improve supply-chain
traceability, workflow/build reproducibility, and release acceptance confidence,
but do not replace native platform signing or a future exact per-target SBOM
predicate.

Before Kaduox-SSH declares the V1 release channel complete, the remaining
publisher-authentication work is primarily:

- Windows Authenticode signing for `.exe` artifacts;
- macOS Developer ID signing and notarization for distributed macOS binaries;
- final operator policy for whether GitHub artifact attestations are enabled on
  the production release repository/account;
- target-specific build-material SBOM attestation only if the release channel
  requires that stronger claim.

Those controls require release credentials or external signing services and
must not be emulated with repository-stored private keys.
