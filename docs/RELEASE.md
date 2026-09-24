# Kaduox-SSH release process

Kaduox-SSH uses a fail-closed release path. A Git tag is not sufficient by itself to publish binaries: repository metadata, tag/version identity, branch reachability, validation policy, the desktop builds, and the final release asset set must pass their gates first.

Distribution is **desktop-only**: the release publishes exactly three assets — the Windows NSIS installer, the macOS Universal pkg, and the standalone MCP server. CLI archives, SPDX SBOMs, checksum manifest files, GitHub attestations, and post-publication qualification were retired; per-asset SHA-256 digests shown on the GitHub Release page cover integrity verification.

## Version preparation

Both workspace crates inherit `[workspace.package].version`. Cargo also records the local package versions in `Cargo.lock`, so changing only `Cargo.toml` breaks the `--locked` contract.

Use the repository release tool instead of editing version strings independently:

```bash
python scripts/release/release_tool.py check
python scripts/release/release_tool.py set-version 0.33.0-rc.1
python scripts/release/release_tool.py check
```

`set-version` computes and validates all metadata edits before the first write. A version-only change should update the workspace version and both local `Cargo.lock` package versions without changing registry dependency versions or checksums.

The desktop client carries its own version copies: `apps/kaduox-desktop/src-tauri/Cargo.toml`, its `Cargo.lock`, `tauri.conf.json`, and `package.json` must be bumped together with the workspace version.

## Required validation before tagging

A release candidate is not ready to tag until the required jobs actually execute successfully:

- Rust 1.98.1 formatting/check/test on Linux, macOS, and Windows;
- `cargo check --workspace --all-targets --locked`;
- `cargo test --workspace --locked`;
- Rust 1.85.0 MSRV check;
- Clippy with warnings denied on Rust 1.98.1;
- dependency audit with pinned `cargo-audit 0.22.0`;
- real OpenSSH integration fixtures;
- immutable external Action pin tests;
- Rust toolchain policy tests;
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
- `.github/workflows/ci.yml`;
- `.github/workflows/quality.yml`;
- `.github/workflows/integration-openssh.yml`;
- `.github/workflows/desktop-macos.yml`.

`scripts/release/test_action_pins.py` enforces the exact approved `action@sha` multiset and occurrence count for each workflow. Mutable tags/branches, abbreviated SHAs, unapproved Actions, missing human-readable version comments, unexpected occurrence counts, and unpinned container Actions fail closed.

Repository-local `./...` actions are tied to the checked-out repository commit and do not require an external Git ref.

## Pinned Rust toolchain policy

Normal CI, Clippy, dependency audit, real OpenSSH integration, and release builds use Rust **1.98.1** explicitly. Rust 1.98.1 replaced 1.98.0 after the Rust project issued the point release for the 1.98.0 vtable-generation miscompilation.

The workspace MSRV remains **Rust 1.85.0** and is tested separately. The audit job installs exactly `cargo-audit 0.22.0` using Rust 1.98.1. The RustSec advisory database is intentionally live security intelligence rather than a frozen build material.

`scripts/release/test_rust_toolchain_policy.py` rejects moving `stable`, changed protected toolchain versions, accidental MSRV changes, and restoration of moving audit toolchains.

## Release asset contract

The release workflow has three jobs after preflight: `desktop-windows`, `desktop-macos`, and `release`.

`desktop-windows` builds the NSIS installer and the MCP server. `desktop-macos` builds the unsigned Universal 2 app and packages it as a `.pkg`. The `release` job downloads both artifact sets and requires **exactly three files**:

```text
Kaduox-SSH-<version>-windows-x64-setup.exe
Kaduox-SSH-<version>-macos-universal.pkg
kaduox-ssh-mcp.exe
```

Any missing or unexpected file fails closed. The job then creates the GitHub Release with curated notes: `docs/releases/v<tag>.md` must exist, its first Markdown heading becomes the release title, and the file becomes the release body. A missing notes file fails closed — no bare auto-generated changelog is published.

Pre-release SemVer tags containing `-` are created as GitHub pre-releases automatically.

Integrity verification relies on the SHA-256 digest GitHub shows for every published asset (for example with PowerShell `Get-FileHash -Algorithm SHA256 <file>`); no separate checksum manifest is published.

## Signing

All desktop packages are currently **unsigned**: Windows may show an unknown-publisher SmartScreen prompt, and macOS requires right-click → Open on first launch. The earlier CLI native-signing pipeline (Authenticode / Developer ID + notarization) was retired together with CLI distribution; the historical design remains documented in `docs/NATIVE_SIGNING.md`.

## Verification after publication

Before announcing a release:

1. confirm the Release page shows exactly the three desktop assets plus GitHub's automatic source archives;
2. download each asset and compare it against the SHA-256 digest shown on the Release page;
3. install the Windows package on a clean profile and smoke-test connect/terminal/SFTP;
4. install the macOS package and confirm the unsigned-launch flow works as documented.

## Retired pipeline elements

The following were part of releases up to v0.33.0-rc.18 and no longer exist:

- four-platform CLI archive matrix (`kaduox-ssh-<version>-<target>.tar.gz/.zip`);
- per-target SPDX SBOMs and the `SHA256SUMS` / `SHA256SUMS.txt` manifests;
- GitHub artifact attestations (`attest` job);
- post-publication qualification (`release-qualification.yml`, `qualification_tool.py`);
- the standalone desktop EXE and the Windows CLI tools ZIP;
- native signing scripts for CLI binaries.

Historical design rationale stays in `docs/V0.20.md`, `docs/V0.24.md`, `docs/V0.27.md`, and `docs/NATIVE_SIGNING.md`.
