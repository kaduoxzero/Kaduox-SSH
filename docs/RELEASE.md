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
python scripts/release/release_tool.py set-version 0.21.0-rc.1
python scripts/release/release_tool.py check
```

`set-version` validates the current repository before writing. It computes the
workspace and both lockfile edits before the first write and refuses unexpected
TOML/lock structure. Review the resulting diff before committing; a version-only
commit should change the workspace version and the two local Cargo.lock package
versions, not registry dependency versions or checksums.

## Required validation before tagging

A release candidate is not ready to tag until all of these execute successfully:

- `cargo fmt --all -- --check`;
- `cargo check --workspace --all-targets --locked` on Linux, macOS, and Windows;
- `cargo test --workspace --locked`;
- Rust 1.85 MSRV check;
- Clippy with warnings denied;
- dependency audit;
- real OpenSSH integration fixtures;
- release packaging and SPDX SBOM Python unit tests;
- immutable release-action pin policy tests;
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

## Immutable release workflow dependencies

The production `.github/workflows/release.yml` must not depend on mutable GitHub
Action tags or branches. Every external `uses:` entry is pinned to a reviewed,
full 40-character Git commit SHA. A human-readable version/ref comment remains
beside each SHA so maintainers can identify the intended upstream release.

`scripts/release/test_action_pins.py` enforces this boundary in both Quality's
`release-policy` job and the tag-release preflight. It rejects mutable refs such
as `@v4` or `@stable`, abbreviated hashes, unapproved external Actions, changed
SHAs that do not match the reviewed approval map, missing version/ref comments,
and unpinned `docker://` actions. Repository-local `./...` actions are permitted
because their contents are already fixed by the checked-out release commit.

Updating an approved release Action is therefore an explicit reviewed change:
update the workflow SHA and the matching approval-map SHA together after
reviewing the upstream commit. The production release workflow independently
reruns this pin policy before publishing a tag.

The v0.21 policy is intentionally scoped to the production release workflow.
Ordinary CI/Quality workflow dependencies remain a separate hardening scope and
do not weaken the tag-release pin check.

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

Pre-release SemVer tags containing `-` (for example `v0.21.0-rc.1`) are created
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

## Verification after publication

Before announcing a release:

1. download all four archives, the SPDX JSON file, and `SHA256SUMS` from the
   GitHub Release;
2. verify all five assets against `SHA256SUMS` using a trusted local SHA-256 tool;
3. inspect each archive's `manifest.json` and ensure tag/version/target are correct;
4. inspect the SPDX document and confirm its tag/version namespace and local
   Kaduox package records;
5. when GitHub attestations are enabled, verify each archive and the SPDX asset
   with `gh attestation verify`;
6. launch `--version` for each executable on representative target machines;
7. run at least one strict host-key SSH connection and one file transfer using
   the published binaries rather than a developer build.

## V1 signing boundary

SHA-256 checksums detect corruption and let users compare bytes against the
published checksum file, but they are **not** platform code signing and do not
by themselves establish publisher identity if the release account is
compromised.

v0.20 provides a deterministic release-wide SPDX dependency inventory and an
optional GitHub provenance-attestation path. v0.21 pins the production release
workflow's external Action dependencies to reviewed immutable commits. These
controls improve supply-chain traceability and workflow integrity but do not
replace native platform signing or a future exact per-target SBOM predicate.

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
