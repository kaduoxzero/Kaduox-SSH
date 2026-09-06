#!/usr/bin/env python3
"""Validate and execute Kaduox-SSH artifacts downloaded from a GitHub Release.

The tool deliberately validates the published bytes rather than a build directory:
SHA256SUMS, the exact release-asset set, archive member safety, package manifest
integrity, and each packaged binary's `--version` result are checked fail-closed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

from scripts.release import release_tool


EXPECTED_BINARIES = release_tool.EXPECTED_BINARIES
EXPECTED_TARGETS: dict[str, tuple[str, str]] = {
    "x86_64-unknown-linux-gnu": ("tar.gz", ""),
    "x86_64-apple-darwin": ("tar.gz", ""),
    "aarch64-apple-darwin": ("tar.gz", ""),
    "x86_64-pc-windows-msvc": ("zip", ".exe"),
}
EXPECTED_DOCUMENTS = ("README.md", "README.zh-CN.md", "LICENSE", "manifest.json")
MAX_CHECKSUM_FILE_BYTES = 64 * 1024
MAX_ARCHIVE_MEMBERS = 32
MAX_MEMBER_BYTES = 128 * 1024 * 1024
MAX_TOTAL_UNCOMPRESSED_BYTES = 512 * 1024 * 1024
MAX_MANIFEST_BYTES = release_tool.MAX_MANIFEST_BYTES
CHECKSUM_LINE = re.compile(r"^(?P<digest>[0-9a-f]{64})  (?P<name>[^\r\n]+)$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def version_from_tag(tag: str) -> str:
    if not tag.startswith("v"):
        raise ValueError("release tag must start with 'v'")
    version = tag[1:]
    if not release_tool.SEMVER.fullmatch(version):
        raise ValueError(f"release tag {tag!r} does not contain a supported SemVer version")
    return version


def package_name(version: str, target: str) -> str:
    release_tool.safe_component(target, "target")
    return release_tool.safe_component(
        f"kaduox-ssh-{version}-{target}", "release package name"
    )


def archive_name(version: str, target: str, archive_format: str) -> str:
    suffix = ".tar.gz" if archive_format == "tar.gz" else ".zip"
    return f"{package_name(version, target)}{suffix}"


def expected_release_assets(tag: str) -> set[str]:
    version = version_from_tag(tag)
    assets = {
        archive_name(version, target, archive_format)
        for target, (archive_format, _exe_suffix) in EXPECTED_TARGETS.items()
    }
    assets.add(f"kaduox-ssh-{version}.spdx.json")
    return assets


def require_plain_file(path: Path, label: str) -> None:
    if path.is_symlink():
        raise ValueError(f"{label} must not be a symbolic link: {path}")
    if not path.is_file():
        raise ValueError(f"{label} is missing or not a regular file: {path}")


def parse_sha256sums(path: Path) -> dict[str, str]:
    require_plain_file(path, "SHA256SUMS")
    size = path.stat().st_size
    if size > MAX_CHECKSUM_FILE_BYTES:
        raise ValueError("SHA256SUMS exceeds the qualification safety budget")
    text = path.read_text(encoding="utf-8")
    found: dict[str, str] = {}
    for line_number, line in enumerate(text.splitlines(), start=1):
        match = CHECKSUM_LINE.fullmatch(line)
        if match is None:
            raise ValueError(f"SHA256SUMS line {line_number} has invalid syntax")
        name = match.group("name")
        if (
            not name
            or name in {".", ".."}
            or "/" in name
            or "\\" in name
            or "\0" in name
            or any(ord(ch) < 32 or ord(ch) == 127 for ch in name)
        ):
            raise ValueError(f"SHA256SUMS line {line_number} has unsafe asset name")
        if name in found:
            raise ValueError(f"SHA256SUMS contains duplicate asset {name!r}")
        found[name] = match.group("digest")
    return found


def verify_release_asset_set(tag: str, assets_dir: Path) -> dict[str, str]:
    if assets_dir.is_symlink() or not assets_dir.is_dir():
        raise ValueError("release assets directory must be a real directory")

    expected = expected_release_assets(tag)
    expected_with_sums = expected | {"SHA256SUMS"}
    actual: set[str] = set()
    for entry in assets_dir.iterdir():
        if entry.name in {".", ".."}:
            raise ValueError("release assets directory contains an unsafe entry")
        require_plain_file(entry, "release asset")
        actual.add(entry.name)
    if actual != expected_with_sums:
        missing = sorted(expected_with_sums - actual)
        unexpected = sorted(actual - expected_with_sums)
        raise ValueError(
            f"published release asset set differs from policy; missing={missing!r}, unexpected={unexpected!r}"
        )

    sums = parse_sha256sums(assets_dir / "SHA256SUMS")
    if set(sums) != expected:
        missing = sorted(expected - set(sums))
        unexpected = sorted(set(sums) - expected)
        raise ValueError(
            f"SHA256SUMS asset set differs from policy; missing={missing!r}, unexpected={unexpected!r}"
        )
    for name in sorted(expected):
        actual_digest = sha256(assets_dir / name)
        if actual_digest != sums[name]:
            raise ValueError(
                f"published asset checksum mismatch for {name}: expected {sums[name]}, got {actual_digest}"
            )
    return sums


def expected_package_files(exe_suffix: str) -> set[str]:
    return {
        *(f"{name}{exe_suffix}" for name in EXPECTED_BINARIES),
        *EXPECTED_DOCUMENTS,
    }


def validate_member_name(name: str, root: str) -> str | None:
    if "\\" in name or "\0" in name:
        raise ValueError(f"archive member has unsafe path syntax: {name!r}")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise ValueError(f"archive member escapes the package root: {name!r}")
    if path.parts[0] != root:
        raise ValueError(f"archive member is outside expected package root {root!r}: {name!r}")
    if len(path.parts) == 1:
        return None
    if len(path.parts) != 2:
        raise ValueError(f"archive member has unexpected nested path: {name!r}")
    return path.parts[1]


def expected_mode(name: str | None, exe_suffix: str) -> int:
    if name is None:
        return 0o755
    binary_files = {f"{binary}{exe_suffix}" for binary in EXPECTED_BINARIES}
    return 0o755 if name in binary_files else 0o644


def validate_declared_size(name: str, size: int, running_total: int) -> int:
    if size < 0 or size > MAX_MEMBER_BYTES:
        raise ValueError(f"archive member {name!r} exceeds the per-file safety budget")
    running_total += size
    if running_total > MAX_TOTAL_UNCOMPRESSED_BYTES:
        raise ValueError("archive exceeds the total uncompressed safety budget")
    return running_total


def _copy_exact(source, destination, expected_size: int) -> None:
    remaining = expected_size
    while remaining:
        chunk = source.read(min(1024 * 1024, remaining))
        if not chunk:
            raise ValueError("archive member ended before its declared byte length")
        destination.write(chunk)
        remaining -= len(chunk)
    if source.read(1):
        raise ValueError("archive member exceeded its declared byte length")


def _prepare_extract_dir(extract_dir: Path) -> None:
    if extract_dir.exists():
        if extract_dir.is_symlink() or not extract_dir.is_dir():
            raise ValueError("qualification extract path must be a real directory")
        if any(extract_dir.iterdir()):
            raise ValueError("qualification extract directory must be empty")
    else:
        extract_dir.mkdir(parents=True)
    if extract_dir.is_symlink():
        raise ValueError("qualification extract directory must not be a symbolic link")


def extract_tar_gz(
    archive: Path, extract_dir: Path, root: str, exe_suffix: str
) -> Path:
    expected_files = expected_package_files(exe_suffix)
    seen_files: set[str] = set()
    root_seen = False
    total = 0
    with tarfile.open(archive, mode="r:gz") as handle:
        members = handle.getmembers()
        if len(members) > MAX_ARCHIVE_MEMBERS:
            raise ValueError("release archive contains too many members")
        for member in members:
            leaf = validate_member_name(member.name, root)
            if member.isdir():
                if leaf is not None or root_seen:
                    raise ValueError(f"release archive contains unexpected directory {member.name!r}")
                root_seen = True
                if stat.S_IMODE(member.mode) != expected_mode(None, exe_suffix):
                    raise ValueError("release archive root directory has unexpected mode")
                continue
            if not member.isfile():
                raise ValueError(f"release archive contains unsupported member type: {member.name!r}")
            if leaf is None or leaf not in expected_files or leaf in seen_files:
                raise ValueError(f"release archive contains unexpected or duplicate file: {member.name!r}")
            if stat.S_IMODE(member.mode) != expected_mode(leaf, exe_suffix):
                raise ValueError(f"release archive member {leaf!r} has unexpected mode")
            total = validate_declared_size(member.name, member.size, total)
            seen_files.add(leaf)

        if not root_seen or seen_files != expected_files:
            raise ValueError("release tar member set differs from the packaging contract")

        package_dir = extract_dir / root
        package_dir.mkdir(mode=0o755)
        for member in members:
            if not member.isfile():
                continue
            leaf = validate_member_name(member.name, root)
            assert leaf is not None
            source = handle.extractfile(member)
            if source is None:
                raise ValueError(f"cannot read archive member {member.name!r}")
            destination = package_dir / leaf
            with source, destination.open("xb") as output:
                _copy_exact(source, output, member.size)
            os.chmod(destination, expected_mode(leaf, exe_suffix))
    return extract_dir / root


def extract_zip(archive: Path, extract_dir: Path, root: str, exe_suffix: str) -> Path:
    expected_files = expected_package_files(exe_suffix)
    seen_files: set[str] = set()
    total = 0
    with zipfile.ZipFile(archive, mode="r") as handle:
        members = handle.infolist()
        if len(members) > MAX_ARCHIVE_MEMBERS:
            raise ValueError("release archive contains too many members")
        for member in members:
            leaf = validate_member_name(member.filename, root)
            if leaf is None or member.is_dir():
                raise ValueError(f"release zip contains unexpected directory: {member.filename!r}")
            if leaf not in expected_files or leaf in seen_files:
                raise ValueError(f"release zip contains unexpected or duplicate file: {member.filename!r}")
            unix_mode = (member.external_attr >> 16) & 0xFFFF
            if stat.S_IFMT(unix_mode) != stat.S_IFREG:
                raise ValueError(f"release zip contains non-regular member: {member.filename!r}")
            if stat.S_IMODE(unix_mode) != expected_mode(leaf, exe_suffix):
                raise ValueError(f"release zip member {leaf!r} has unexpected mode")
            total = validate_declared_size(member.filename, member.file_size, total)
            seen_files.add(leaf)
        if seen_files != expected_files:
            raise ValueError("release zip member set differs from the packaging contract")

        package_dir = extract_dir / root
        package_dir.mkdir(mode=0o755)
        for member in members:
            leaf = validate_member_name(member.filename, root)
            assert leaf is not None
            destination = package_dir / leaf
            with handle.open(member, mode="r") as source, destination.open("xb") as output:
                _copy_exact(source, output, member.file_size)
            os.chmod(destination, expected_mode(leaf, exe_suffix))
    return extract_dir / root


def extract_release_archive(
    archive: Path,
    extract_dir: Path,
    version: str,
    target: str,
    archive_format: str,
    exe_suffix: str,
) -> Path:
    require_plain_file(archive, "release archive")
    _prepare_extract_dir(extract_dir)
    root = package_name(version, target)
    if archive_format == "tar.gz":
        return extract_tar_gz(archive, extract_dir, root, exe_suffix)
    if archive_format == "zip":
        return extract_zip(archive, extract_dir, root, exe_suffix)
    raise ValueError(f"unsupported release archive format: {archive_format}")


def validate_manifest(
    package_dir: Path, tag: str, version: str, target: str, exe_suffix: str
) -> None:
    manifest_path = package_dir / "manifest.json"
    require_plain_file(manifest_path, "release manifest")
    if manifest_path.stat().st_size > MAX_MANIFEST_BYTES:
        raise ValueError("release manifest exceeds the safety budget")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError("release manifest is not valid JSON") from exc
    if not isinstance(manifest, dict) or set(manifest) != {
        "schema",
        "project",
        "tag",
        "version",
        "target",
        "binaries",
    }:
        raise ValueError("release manifest has an unexpected top-level schema")
    if manifest["schema"] != 1 or manifest["project"] != "Kaduox-SSH":
        raise ValueError("release manifest identity/schema mismatch")
    if manifest["tag"] != tag or manifest["version"] != version or manifest["target"] != target:
        raise ValueError("release manifest tag/version/target mismatch")
    binaries = manifest["binaries"]
    if not isinstance(binaries, list) or len(binaries) != len(EXPECTED_BINARIES):
        raise ValueError("release manifest binary set has unexpected size")

    by_name: dict[str, dict] = {}
    for entry in binaries:
        if not isinstance(entry, dict) or set(entry) != {"name", "file", "sha256", "bytes"}:
            raise ValueError("release manifest binary entry has unexpected schema")
        name = entry.get("name")
        if not isinstance(name, str) or name not in EXPECTED_BINARIES or name in by_name:
            raise ValueError("release manifest contains an unknown or duplicate binary")
        by_name[name] = entry

    if set(by_name) != set(EXPECTED_BINARIES):
        raise ValueError("release manifest is missing required binaries")
    for name in EXPECTED_BINARIES:
        entry = by_name[name]
        expected_file = f"{name}{exe_suffix}"
        if entry["file"] != expected_file:
            raise ValueError(f"release manifest filename mismatch for {name}")
        if not isinstance(entry["bytes"], int) or entry["bytes"] < 0:
            raise ValueError(f"release manifest byte length is invalid for {name}")
        if not isinstance(entry["sha256"], str) or re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]) is None:
            raise ValueError(f"release manifest checksum is invalid for {name}")
        binary = package_dir / expected_file
        require_plain_file(binary, f"packaged binary {name}")
        if binary.stat().st_size != entry["bytes"]:
            raise ValueError(f"packaged binary size mismatch for {name}")
        if sha256(binary) != entry["sha256"]:
            raise ValueError(f"packaged binary checksum mismatch for {name}")


def run_version_smoke(package_dir: Path, version: str, exe_suffix: str) -> None:
    for name in EXPECTED_BINARIES:
        binary = package_dir / f"{name}{exe_suffix}"
        try:
            completed = subprocess.run(
                [str(binary), "--version"],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise ValueError(f"failed to launch published binary {name}: {exc}") from exc
        output = completed.stdout.strip()
        if completed.returncode != 0:
            raise ValueError(
                f"published binary {name} --version exited {completed.returncode}: {output!r}"
            )
        if version not in output:
            raise ValueError(
                f"published binary {name} --version does not contain expected version {version!r}: {output!r}"
            )


def qualify_target(
    tag: str,
    target: str,
    archive_format: str,
    exe_suffix: str,
    assets_dir: Path,
    extract_dir: Path,
) -> Path:
    version = version_from_tag(tag)
    expected_target = EXPECTED_TARGETS.get(target)
    if expected_target is None:
        raise ValueError(f"unsupported release qualification target: {target}")
    if (archive_format, exe_suffix) != expected_target:
        raise ValueError(
            f"target {target} requires format/suffix {expected_target!r}, got {(archive_format, exe_suffix)!r}"
        )
    verify_release_asset_set(tag, assets_dir)
    archive = assets_dir / archive_name(version, target, archive_format)
    package_dir = extract_release_archive(
        archive, extract_dir, version, target, archive_format, exe_suffix
    )
    validate_manifest(package_dir, tag, version, target, exe_suffix)
    run_version_smoke(package_dir, version, exe_suffix)
    return package_dir


def command_verify_set(args: argparse.Namespace) -> int:
    verify_release_asset_set(args.tag, Path(args.assets_dir).resolve())
    print("release asset set verified")
    return 0


def command_qualify(args: argparse.Namespace) -> int:
    package_dir = qualify_target(
        tag=args.tag,
        target=args.target,
        archive_format=args.format,
        exe_suffix=args.exe_suffix,
        assets_dir=Path(args.assets_dir).resolve(),
        extract_dir=Path(args.extract_dir).resolve(),
    )
    print(package_dir)
    return 0


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    sub = root.add_subparsers(dest="command", required=True)

    verify = sub.add_parser("verify-set", help="verify the complete downloaded release asset set")
    verify.add_argument("--tag", required=True)
    verify.add_argument("--assets-dir", required=True)
    verify.set_defaults(func=command_verify_set)

    qualify = sub.add_parser("qualify", help="verify, safely extract, and smoke-test one target archive")
    qualify.add_argument("--tag", required=True)
    qualify.add_argument("--target", required=True)
    qualify.add_argument("--format", choices=("tar.gz", "zip"), required=True)
    qualify.add_argument("--exe-suffix", default="")
    qualify.add_argument("--assets-dir", required=True)
    qualify.add_argument("--extract-dir", required=True)
    qualify.set_defaults(func=command_qualify)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        return args.func(args)
    except (OSError, ValueError, tarfile.TarError, zipfile.BadZipFile) as exc:
        print(f"release qualification failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
