# Kaduox-SSH release process

Kaduox-SSH uses a fail-closed release path. A Git tag is not sufficient by
itself to publish binaries: repository metadata, tag/version identity, branch
reachability, all target builds, archive contents, and the final archive set
must pass their gates first.

## Version preparation

Both workspace crates inherit `[workspace.package].version`. Cargo also records
the local package versions in `Cargo.lock`, so changing only `Cargo.toml` breaks
the `--locked` contract.

Use the repository release tool instead of editing the version strings by hand:

```bash
python scripts/release/release_tool.py check
python scripts/release/release_tool.py set-version 0.13.0-rc.1
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
- release-policy Python unit tests and repository metadata validation.

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

## Release artifacts

The release matrix currently produces four suites:

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
The publish job refuses any release with anything other than exactly four final
archives and creates a top-level `SHA256SUMS` covering those archives.

Pre-release SemVer tags containing `-` (for example `v0.13.0-rc.1`) are created
as GitHub pre-releases automatically.

## Verification after publication

Before announcing a release:

1. download all four archives and `SHA256SUMS` from the GitHub Release;
2. verify every archive against `SHA256SUMS` using a trusted local SHA-256 tool;
3. inspect each `manifest.json` and ensure tag/version/target are correct;
4. launch `--version` for each executable on representative target machines;
5. run at least one strict host-key SSH connection and one file transfer using
   the published binaries rather than a developer build.

## V1 signing boundary

SHA-256 checksums detect corruption and let users compare bytes against the
published checksum file, but they are **not** platform code signing and do not
by themselves establish publisher identity if the release account is
compromised.

Before Kaduox-SSH declares the V1 release channel complete, the release plan
should decide and implement the appropriate publisher-authentication layer:

- Windows Authenticode signing for `.exe` artifacts;
- macOS Developer ID signing and notarization for distributed macOS binaries;
- provenance/attestation and an SBOM policy where useful for the distribution
  channel.

Those controls require release credentials or external signing services and
must not be emulated with repository-stored private keys.
