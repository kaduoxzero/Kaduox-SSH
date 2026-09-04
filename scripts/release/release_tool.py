#!/usr/bin/env python3
"""Release preflight and packaging helpers for Kaduox-SSH.

Uses only the Python standard library so GitHub-hosted release jobs can run the
same validation and packaging logic on Linux, macOS, and Windows.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXPECTED_BINARIES = ("kssh", "kssh-tui", "kssh-fleet", "kssh-inventory")
MAX_MANIFEST_PATH_BYTES = 16 * 1024


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def workspace_version() -> str:
    data = load_toml(ROOT / "Cargo.toml")
    try:
        version = data["workspace"]["package"]["version"]
    except (KeyError, TypeError) as exc:
        raise ValueError("Cargo.toml is missing [workspace.package].version") from exc
    if not isinstance(version, str) or not version.strip():
        raise ValueError("workspace package version must be a non-empty string")
    return version


def declared_binaries() -> tuple[str, ...]:
    data = load_toml(ROOT / "crates" / "kaduox-ssh-cli" / "Cargo.toml")
    bins = data.get("bin", [])
    names: list[str] = []
    for entry in bins:
        if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
            raise ValueError("every [[bin]] entry must have a string name")
        names.append(entry["name"])
    return tuple(names)


def validate_repository() -> str:
    version = workspace_version()
    actual = declared_binaries()
    if actual != EXPECTED_BINARIES:
        raise ValueError(
            "release binary declarations changed: "
            f"expected {EXPECTED_BINARIES!r}, got {actual!r}; update release policy explicitly"
        )

    for crate in ("kaduox-ssh-core", "kaduox-ssh-cli"):
        data = load_toml(ROOT / "crates" / crate / "Cargo.toml")
        package = data.get("package", {})
        if package.get("version", {}).get("workspace") is not True:
            raise ValueError(f"{crate} must inherit version.workspace = true")
        if package.get("rust-version", {}).get("workspace") is not True:
            raise ValueError(f"{crate} must inherit rust-version.workspace = true")
    return version


def check_tag(tag: str) -> str:
    version = validate_repository()
    expected = f"v{version}"
    if tag != expected:
        raise ValueError(
            f"release tag {tag!r} does not match workspace version {version!r}; "
            f"expected exactly {expected!r}"
        )
    return version


def safe_component(value: str, label: str) -> str:
    if not value or value in {".", ".."}:
        raise ValueError(f"{label} must be non-empty")
    if any(ch in value for ch in ("/", "\\", "\0")):
        raise ValueError(f"{label} must be a single path component")
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        raise ValueError(f"{label} cannot contain control characters")
    if len(value.encode("utf-8")) > 256:
        raise ValueError(f"{label} is too long")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def binary_source(target: str, binary: str, exe_suffix: str) -> Path:
    return ROOT / "target" / target / "release" / f"{binary}{exe_suffix}"


def build_manifest(tag: str, target: str, package_dir: Path, exe_suffix: str) -> dict:
    entries = []
    for binary in EXPECTED_BINARIES:
        path = package_dir / f"{binary}{exe_suffix}"
        entries.append(
            {
                "name": binary,
                "file": path.name,
                "sha256": sha256(path),
                "bytes": path.stat().st_size,
            }
        )
    return {
        "schema": 1,
        "project": "Kaduox-SSH",
        "tag": tag,
        "version": workspace_version(),
        "target": target,
        "binaries": entries,
    }


def create_archive(source_dir: Path, archive: Path, archive_format: str) -> None:
    if archive_format == "tar.gz":
        with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as output:
            output.add(source_dir, arcname=source_dir.name)
        return
    if archive_format == "zip":
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
            for path in sorted(source_dir.rglob("*")):
                if path.is_file():
                    output.write(path, arcname=Path(source_dir.name) / path.relative_to(source_dir))
        return
    raise ValueError(f"unsupported archive format: {archive_format}")


def package_release(tag: str, target: str, archive_format: str, exe_suffix: str, output_dir: Path) -> Path:
    version = check_tag(tag)
    target = safe_component(target, "target")
    exe_suffix = safe_component(exe_suffix, "exe suffix") if exe_suffix else ""
    if exe_suffix not in {"", ".exe"}:
        raise ValueError("exe suffix must be empty or .exe")

    output_dir.mkdir(parents=True, exist_ok=True)
    package_name = safe_component(f"kaduox-ssh-{version}-{target}", "package name")
    archive_suffix = ".tar.gz" if archive_format == "tar.gz" else ".zip"
    archive = output_dir / f"{package_name}{archive_suffix}"

    with tempfile.TemporaryDirectory(prefix="kaduox-release-") as temp:
        package_dir = Path(temp) / package_name
        package_dir.mkdir()

        for binary in EXPECTED_BINARIES:
            source = binary_source(target, binary, exe_suffix)
            if not source.is_file():
                raise FileNotFoundError(f"missing release binary: {source}")
            shutil.copy2(source, package_dir / source.name)

        for document in ("README.md", "README.zh-CN.md", "LICENSE"):
            source = ROOT / document
            if not source.is_file():
                raise FileNotFoundError(f"missing release document: {source}")
            shutil.copy2(source, package_dir / document)

        manifest = build_manifest(tag, target, package_dir, exe_suffix)
        manifest_text = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
        if len(manifest_text.encode("utf-8")) > MAX_MANIFEST_PATH_BYTES:
            raise ValueError("release manifest unexpectedly exceeds safety budget")
        (package_dir / "manifest.json").write_text(manifest_text, encoding="utf-8")
        create_archive(package_dir, archive, archive_format)

    return archive


def command_check(args: argparse.Namespace) -> int:
    version = validate_repository()
    if args.tag:
        check_tag(args.tag)
    print(version)
    return 0


def command_package(args: argparse.Namespace) -> int:
    archive = package_release(
        tag=args.tag,
        target=args.target,
        archive_format=args.format,
        exe_suffix=args.exe_suffix,
        output_dir=Path(args.output_dir).resolve(),
    )
    print(archive)
    return 0


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    sub = root.add_subparsers(dest="command", required=True)

    check = sub.add_parser("check", help="validate release metadata")
    check.add_argument("--tag", help="require an exact v<workspace-version> tag")
    check.set_defaults(func=command_check)

    package = sub.add_parser("package", help="stage and archive all release binaries")
    package.add_argument("--tag", required=True)
    package.add_argument("--target", required=True)
    package.add_argument("--format", choices=("tar.gz", "zip"), required=True)
    package.add_argument("--exe-suffix", default="")
    package.add_argument("--output-dir", default="dist")
    package.set_defaults(func=command_package)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        return args.func(args)
    except (OSError, ValueError) as exc:
        print(f"release validation failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
