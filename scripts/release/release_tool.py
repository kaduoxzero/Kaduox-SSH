#!/usr/bin/env python3
"""Release preflight, version synchronization, and packaging for Kaduox-SSH.

Uses only the Python standard library so GitHub-hosted release jobs can run the
same validation and packaging logic on Linux, macOS, and Windows.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXPECTED_BINARIES = ("kssh", "kssh-tui", "kssh-fleet", "kssh-inventory")
LOCAL_PACKAGES = ("kaduox-ssh-core", "kaduox-ssh-cli")
MAX_MANIFEST_BYTES = 16 * 1024
SEMVER = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def workspace_version() -> str:
    data = load_toml(ROOT / "Cargo.toml")
    try:
        version = data["workspace"]["package"]["version"]
    except (KeyError, TypeError) as exc:
        raise ValueError("Cargo.toml is missing [workspace.package].version") from exc
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        raise ValueError("workspace package version must use the supported SemVer syntax")
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


def lock_local_versions() -> dict[str, str]:
    data = load_toml(ROOT / "Cargo.lock")
    found: dict[str, str] = {}
    for package in data.get("package", []):
        name = package.get("name")
        if name in LOCAL_PACKAGES:
            if name in found:
                raise ValueError(f"Cargo.lock contains duplicate local package {name}")
            version = package.get("version")
            if not isinstance(version, str):
                raise ValueError(f"Cargo.lock local package {name} has no string version")
            found[name] = version
    missing = [name for name in LOCAL_PACKAGES if name not in found]
    if missing:
        raise ValueError(f"Cargo.lock is missing local package records: {', '.join(missing)}")
    return found


def validate_repository() -> str:
    version = workspace_version()
    actual = declared_binaries()
    if actual != EXPECTED_BINARIES:
        raise ValueError(
            "release binary declarations changed: "
            f"expected {EXPECTED_BINARIES!r}, got {actual!r}; update release policy explicitly"
        )

    for crate in LOCAL_PACKAGES:
        data = load_toml(ROOT / "crates" / crate / "Cargo.toml")
        package = data.get("package", {})
        if package.get("version", {}).get("workspace") is not True:
            raise ValueError(f"{crate} must inherit version.workspace = true")
        if package.get("rust-version", {}).get("workspace") is not True:
            raise ValueError(f"{crate} must inherit rust-version.workspace = true")

    for crate, locked_version in lock_local_versions().items():
        if locked_version != version:
            raise ValueError(
                f"Cargo.lock local package {crate} is {locked_version}, but workspace version is {version}; "
                "run release_tool.py set-version to update both together"
            )
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


def replace_workspace_version(text: str, old: str, new: str) -> str:
    lines = text.splitlines(keepends=True)
    in_workspace_package = False
    replaced = 0
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_workspace_package = stripped == "[workspace.package]"
            continue
        if in_workspace_package and stripped.startswith("version ="):
            expected = f'version = "{old}"'
            if stripped != expected:
                raise ValueError(
                    f"unexpected workspace version line {stripped!r}; expected {expected!r}"
                )
            newline = "\r\n" if line.endswith("\r\n") else "\n" if line.endswith("\n") else ""
            indent = line[: len(line) - len(line.lstrip())]
            lines[index] = f'{indent}version = "{new}"{newline}'
            replaced += 1
            break
    if replaced != 1:
        raise ValueError("failed to locate exactly one [workspace.package] version line")
    return "".join(lines)


def replace_lock_versions(text: str, old: str, new: str) -> str:
    output = text
    for package in LOCAL_PACKAGES:
        marker = f'name = "{package}"\nversion = "{old}"'
        count = output.count(marker)
        if count != 1:
            raise ValueError(
                f"expected exactly one Cargo.lock {package} record at version {old}, found {count}"
            )
        output = output.replace(marker, f'name = "{package}"\nversion = "{new}"', 1)
    return output


def write_text_safely(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    original_mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o644
    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temp_name, original_mode)
        os.replace(temp_name, path)
    except BaseException:
        try:
            os.unlink(temp_name)
        except FileNotFoundError:
            pass
        raise


def set_version(new_version: str) -> tuple[str, str]:
    if not SEMVER.fullmatch(new_version):
        raise ValueError(f"new version {new_version!r} does not use the supported SemVer syntax")
    old_version = validate_repository()
    if new_version == old_version:
        return old_version, new_version

    cargo_path = ROOT / "Cargo.toml"
    lock_path = ROOT / "Cargo.lock"
    cargo_text = cargo_path.read_text(encoding="utf-8")
    lock_text = lock_path.read_text(encoding="utf-8")

    # Compute and validate every edit before the first write. This prevents an
    # input/formatting error from leaving only one metadata file changed.
    new_cargo = replace_workspace_version(cargo_text, old_version, new_version)
    new_lock = replace_lock_versions(lock_text, old_version, new_version)

    write_text_safely(lock_path, new_lock)
    try:
        write_text_safely(cargo_path, new_cargo)
    except BaseException:
        # Best-effort rollback keeps the normal failure path consistent if the
        # second metadata replacement fails after Cargo.lock was committed.
        write_text_safely(lock_path, lock_text)
        raise
    return old_version, new_version


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
        if len(manifest_text.encode("utf-8")) > MAX_MANIFEST_BYTES:
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


def command_set_version(args: argparse.Namespace) -> int:
    old, new = set_version(args.version)
    print(f"{old} -> {new}")
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

    set_version_parser = sub.add_parser(
        "set-version", help="update workspace and Cargo.lock local package versions together"
    )
    set_version_parser.add_argument("version")
    set_version_parser.set_defaults(func=command_set_version)

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
