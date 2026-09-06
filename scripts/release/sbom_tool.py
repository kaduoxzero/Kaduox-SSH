#!/usr/bin/env python3
"""Generate deterministic target-specific SPDX 2.3 release SBOMs."""

from __future__ import annotations

import argparse
from collections import deque
from datetime import datetime, timezone
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from urllib.parse import quote

try:
    from scripts.release import release_tool
except ModuleNotFoundError:  # Direct `python scripts/release/sbom_tool.py` execution.
    import release_tool  # type: ignore[no-redef]

ROOT = release_tool.ROOT
MAX_SBOM_BYTES = 8 * 1024 * 1024
MAX_METADATA_BYTES = 16 * 1024 * 1024
SPDX_VERSION = "SPDX-2.3"
SPDX_DATA_LICENSE = "CC0-1.0"
DEFAULT_SOURCE_DATE_EPOCH = 0
RELEASE_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


def validate_target(target: str) -> str:
    release_tool.safe_component(target, "release target")
    if target not in RELEASE_TARGETS:
        raise ValueError(
            f"unsupported SBOM release target {target!r}; expected one of {RELEASE_TARGETS!r}"
        )
    return target


def load_lock(path: Path | None = None) -> dict:
    lock_path = path or ROOT / "Cargo.lock"
    with lock_path.open("rb") as handle:
        data = tomllib.load(handle)
    if data.get("version") != 4:
        raise ValueError("SBOM generation requires Cargo.lock format version 4")
    packages = data.get("package")
    if not isinstance(packages, list) or not packages:
        raise ValueError("Cargo.lock contains no package records")
    return data


def lock_package_identity(package: dict) -> tuple[str, str, str]:
    name = package.get("name")
    version = package.get("version")
    source = package.get("source", "")
    if not isinstance(name, str) or not name:
        raise ValueError("Cargo.lock package has an invalid name")
    if not isinstance(version, str) or not version:
        raise ValueError(f"Cargo.lock package {name!r} has an invalid version")
    if not isinstance(source, str):
        raise ValueError(f"Cargo.lock package {name!r} has a non-string source")
    return name, version, source


def metadata_package_identity(package: dict) -> tuple[str, str, str]:
    name = package.get("name")
    version = package.get("version")
    source = package.get("source")
    if not isinstance(name, str) or not name:
        raise ValueError("Cargo metadata package has an invalid name")
    if not isinstance(version, str) or not version:
        raise ValueError(f"Cargo metadata package {name!r} has an invalid version")
    if source is None:
        source = ""
    if not isinstance(source, str):
        raise ValueError(f"Cargo metadata package {name!r} has an invalid source")
    return name, version, source


def spdx_id_for(package: dict) -> str:
    name, version, source = lock_package_identity(package)
    digest = hashlib.sha256(f"{name}\0{version}\0{source}".encode()).hexdigest()[:16]
    safe_name = re.sub(r"[^A-Za-z0-9.-]", "-", name)
    return f"SPDXRef-Package-{safe_name}-{digest}"


def purl_for(package: dict) -> str:
    name, version, _source = lock_package_identity(package)
    return f"pkg:cargo/{quote(name, safe='-._~')}@{quote(version, safe='-._~')}"


def spdx_package(package: dict) -> dict:
    name, version, source = lock_package_identity(package)
    result = {
        "SPDXID": spdx_id_for(package),
        "name": name,
        "versionInfo": version,
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": False,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "copyrightText": "NOASSERTION",
        "externalRefs": [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": purl_for(package),
            }
        ],
    }
    checksum = package.get("checksum")
    if checksum is not None:
        if not isinstance(checksum, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", checksum):
            raise ValueError(f"Cargo.lock package {name!r} has an invalid SHA-256 checksum")
        result["checksums"] = [
            {"algorithm": "SHA256", "checksumValue": checksum.lower()}
        ]
    if source:
        result["sourceInfo"] = f"Cargo.lock source: {source}"
    return result


def spdx_created_from_epoch(value: str | int) -> str:
    try:
        epoch = int(value)
    except (TypeError, ValueError) as exc:
        raise ValueError("source date epoch must be an integer number of seconds") from exc
    if epoch < 0:
        raise ValueError("source date epoch cannot be negative")
    try:
        instant = datetime.fromtimestamp(epoch, timezone.utc)
    except (OverflowError, OSError, ValueError) as exc:
        raise ValueError("source date epoch is outside the supported timestamp range") from exc
    return instant.strftime("%Y-%m-%dT%H:%M:%SZ")


def run_cargo_metadata(target: str) -> dict:
    target = validate_target(target)
    command = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--filter-platform",
        target,
    ]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        stderr = completed.stderr[-8192:].decode("utf-8", errors="replace")
        raise ValueError(
            f"cargo metadata failed for target {target!r} with exit code {completed.returncode}: {stderr.strip()}"
        )
    if len(completed.stdout) > MAX_METADATA_BYTES:
        raise ValueError("cargo metadata output exceeds the 16 MiB safety limit")
    try:
        data = json.loads(completed.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError("cargo metadata did not return valid JSON") from exc
    if not isinstance(data, dict):
        raise ValueError("cargo metadata root must be a JSON object")
    return data


def _metadata_indexes(metadata: dict) -> tuple[dict[str, dict], dict[str, dict]]:
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages, list) or not packages:
        raise ValueError("cargo metadata contains no packages")
    if not isinstance(resolve, dict):
        raise ValueError("cargo metadata is missing the resolved dependency graph")
    nodes = resolve.get("nodes")
    if not isinstance(nodes, list) or not nodes:
        raise ValueError("cargo metadata resolve graph contains no nodes")

    packages_by_id: dict[str, dict] = {}
    for package in packages:
        if not isinstance(package, dict):
            raise ValueError("cargo metadata package entry must be an object")
        package_id = package.get("id")
        if not isinstance(package_id, str) or not package_id:
            raise ValueError("cargo metadata package has an invalid package id")
        metadata_package_identity(package)
        if package_id in packages_by_id:
            raise ValueError(f"cargo metadata contains duplicate package id {package_id!r}")
        packages_by_id[package_id] = package

    nodes_by_id: dict[str, dict] = {}
    for node in nodes:
        if not isinstance(node, dict):
            raise ValueError("cargo metadata resolve node must be an object")
        package_id = node.get("id")
        if not isinstance(package_id, str) or package_id not in packages_by_id:
            raise ValueError("cargo metadata resolve node references an unknown package id")
        if package_id in nodes_by_id:
            raise ValueError(f"cargo metadata contains duplicate resolve node {package_id!r}")
        nodes_by_id[package_id] = node
    return packages_by_id, nodes_by_id


def release_graph(metadata: dict) -> tuple[set[str], set[tuple[str, str, str]]]:
    packages_by_id, nodes_by_id = _metadata_indexes(metadata)
    workspace_members = metadata.get("workspace_members")
    if not isinstance(workspace_members, list):
        raise ValueError("cargo metadata workspace_members must be a list")
    if any(not isinstance(item, str) for item in workspace_members):
        raise ValueError("cargo metadata workspace member id must be a string")
    if len(workspace_members) != len(set(workspace_members)):
        raise ValueError("cargo metadata contains duplicate workspace member ids")
    if len(workspace_members) != len(release_tool.LOCAL_PACKAGES):
        raise ValueError("cargo metadata workspace member count changed")
    if any(package_id not in packages_by_id for package_id in workspace_members):
        raise ValueError("cargo metadata workspace member references an unknown package id")

    workspace_names = {
        packages_by_id[package_id].get("name") for package_id in workspace_members
    }
    if workspace_names != set(release_tool.LOCAL_PACKAGES):
        raise ValueError(
            f"cargo metadata workspace package set changed: {workspace_names!r}"
        )
    cli_roots = [
        package_id
        for package_id in workspace_members
        if packages_by_id[package_id].get("name") == "kaduox-ssh-cli"
    ]
    if len(cli_roots) != 1:
        raise ValueError("cargo metadata must contain exactly one kaduox-ssh-cli workspace package")

    reachable: set[str] = set()
    relationships: set[tuple[str, str, str]] = set()
    pending = deque(cli_roots)
    while pending:
        package_id = pending.popleft()
        if package_id in reachable:
            continue
        node = nodes_by_id.get(package_id)
        if node is None:
            raise ValueError(f"cargo metadata is missing resolve node for {package_id!r}")
        reachable.add(package_id)
        deps = node.get("deps")
        if not isinstance(deps, list):
            raise ValueError(f"cargo metadata node {package_id!r} has invalid deps")
        for dep in deps:
            if not isinstance(dep, dict):
                raise ValueError("cargo metadata dependency edge must be an object")
            dep_id = dep.get("pkg")
            dep_kinds = dep.get("dep_kinds")
            if not isinstance(dep_id, str) or dep_id not in packages_by_id:
                raise ValueError("cargo metadata dependency edge references an unknown package")
            if not isinstance(dep_kinds, list) or not dep_kinds:
                raise ValueError("cargo metadata dependency edge has no dependency kinds")

            kinds: set[str | None] = set()
            for item in dep_kinds:
                if not isinstance(item, dict):
                    raise ValueError("cargo metadata dependency kind must be an object")
                kind = item.get("kind")
                if kind not in {None, "build", "dev"}:
                    raise ValueError(f"unsupported Cargo dependency kind {kind!r}")
                kinds.add(kind)
            if kinds <= {"dev"}:
                continue
            if None in kinds:
                relationships.add((package_id, "DEPENDS_ON", dep_id))
            if "build" in kinds:
                relationships.add((dep_id, "BUILD_DEPENDENCY_OF", package_id))
            pending.append(dep_id)

    return reachable, relationships


def _lock_packages_by_identity(lock_data: dict) -> dict[tuple[str, str, str], dict]:
    packages = lock_data.get("package")
    if not isinstance(packages, list) or not packages:
        raise ValueError("Cargo.lock contains no package records")
    indexed: dict[tuple[str, str, str], dict] = {}
    for package in packages:
        if not isinstance(package, dict):
            raise ValueError("Cargo.lock package entry must be an object")
        identity = lock_package_identity(package)
        if identity in indexed:
            raise ValueError(f"Cargo.lock contains duplicate package identity {identity!r}")
        indexed[identity] = package
    return indexed


def build_spdx_document(
    lock_data: dict,
    metadata: dict,
    tag: str,
    target: str,
    created: str,
) -> dict:
    version = release_tool.check_tag(tag)
    target = validate_target(target)
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", created):
        raise ValueError("SPDX creation timestamp must use UTC YYYY-MM-DDThh:mm:ssZ format")

    packages_by_id, _nodes_by_id = _metadata_indexes(metadata)
    reachable, metadata_relationships = release_graph(metadata)
    lock_by_identity = _lock_packages_by_identity(lock_data)

    reachable_identities = [
        metadata_package_identity(packages_by_id[package_id])
        for package_id in sorted(reachable)
    ]
    if len(reachable_identities) != len(set(reachable_identities)):
        raise ValueError("target-resolved Cargo graph contains duplicate package identities")

    lock_for_metadata_id: dict[str, dict] = {}
    for package_id in sorted(reachable):
        identity = metadata_package_identity(packages_by_id[package_id])
        package = lock_by_identity.get(identity)
        if package is None:
            raise ValueError(
                f"target-resolved Cargo package {identity!r} is missing from Cargo.lock"
            )
        lock_for_metadata_id[package_id] = package

    ordered_lock_packages = sorted(lock_for_metadata_id.values(), key=lock_package_identity)
    root_spdx_id = "SPDXRef-Kaduox-SSH-Release"
    spdx_packages = [
        {
            "SPDXID": root_spdx_id,
            "name": "Kaduox-SSH",
            "versionInfo": version,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": "NOASSERTION",
            "copyrightText": "NOASSERTION",
            "primaryPackagePurpose": "APPLICATION",
            "comment": f"Cargo release target: {target}",
        }
    ]
    spdx_packages.extend(spdx_package(package) for package in ordered_lock_packages)

    relationships: set[tuple[str, str, str]] = {
        ("SPDXRef-DOCUMENT", "DESCRIBES", root_spdx_id)
    }
    for source_id, relationship, destination_id in metadata_relationships:
        relationships.add(
            (
                spdx_id_for(lock_for_metadata_id[source_id]),
                relationship,
                spdx_id_for(lock_for_metadata_id[destination_id]),
            )
        )

    cli_ids = [
        package_id
        for package_id, package in packages_by_id.items()
        if package_id in reachable and package.get("name") == "kaduox-ssh-cli"
    ]
    if len(cli_ids) != 1:
        raise ValueError("target SBOM graph must contain exactly one kaduox-ssh-cli root")
    relationships.add(
        (root_spdx_id, "DEPENDS_ON", spdx_id_for(lock_for_metadata_id[cli_ids[0]]))
    )

    local_names = {
        lock_package_identity(package)[0]
        for package in ordered_lock_packages
        if lock_package_identity(package)[2] == ""
    }
    if not set(release_tool.LOCAL_PACKAGES).issubset(local_names):
        raise ValueError("target SBOM graph does not contain every local Kaduox package")

    namespace = (
        "https://github.com/kaduoxzero/Kaduox-SSH/sbom/"
        f"{quote(tag, safe='-._~')}/{quote(target, safe='-._~')}/cargo-metadata"
    )
    return {
        "spdxVersion": SPDX_VERSION,
        "dataLicense": SPDX_DATA_LICENSE,
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"Kaduox-SSH-{version}-{target}-Cargo-metadata",
        "documentNamespace": namespace,
        "creationInfo": {
            "created": created,
            "creators": ["Tool: Kaduox-SSH scripts/release/sbom_tool.py"],
            "comment": (
                f"Target-specific Cargo dependency/build-material graph for {target}. "
                "The graph is generated with cargo metadata --locked --filter-platform for this "
                "release target; dev-only dependency edges are excluded, normal dependencies use "
                "DEPENDS_ON, and build dependencies use BUILD_DEPENDENCY_OF. The creation timestamp "
                "is derived from the tagged source commit for reproducible output."
            ),
        },
        "packages": spdx_packages,
        "relationships": [
            {
                "spdxElementId": source,
                "relationshipType": relationship,
                "relatedSpdxElement": destination,
            }
            for source, relationship, destination in sorted(relationships)
        ],
    }


def render_spdx(document: dict) -> str:
    text = json.dumps(document, indent=2, sort_keys=True) + "\n"
    if len(text.encode("utf-8")) > MAX_SBOM_BYTES:
        raise ValueError("generated SPDX SBOM exceeds the 8 MiB safety limit")
    return text


def generate_sbom(
    tag: str,
    target: str,
    output_dir: Path,
    source_date_epoch: str | int = DEFAULT_SOURCE_DATE_EPOCH,
    metadata: dict | None = None,
) -> Path:
    version = release_tool.check_tag(tag)
    target = validate_target(target)
    created = spdx_created_from_epoch(source_date_epoch)
    metadata = metadata if metadata is not None else run_cargo_metadata(target)
    document = build_spdx_document(load_lock(), metadata, tag, target, created)
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / f"kaduox-ssh-{version}-{target}.spdx.json"
    release_tool.write_text_safely(output, render_spdx(document))
    return output


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--tag", required=True)
    root.add_argument("--target", required=True, choices=RELEASE_TARGETS)
    root.add_argument("--source-date-epoch", default=str(DEFAULT_SOURCE_DATE_EPOCH))
    root.add_argument("--output-dir", default="dist")
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        output = generate_sbom(
            args.tag,
            args.target,
            Path(args.output_dir).resolve(),
            args.source_date_epoch,
        )
    except (OSError, ValueError) as exc:
        print(f"SBOM generation failed: {exc}", file=sys.stderr)
        return 2
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
