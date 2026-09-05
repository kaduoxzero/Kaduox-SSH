#!/usr/bin/env python3
"""Generate a deterministic SPDX 2.3 SBOM from the resolved Cargo.lock graph."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
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
SPDX_VERSION = "SPDX-2.3"
SPDX_DATA_LICENSE = "CC0-1.0"
NORMALIZED_CREATED = "1970-01-01T00:00:00Z"
DEPENDENCY_SPEC = re.compile(r"^(?P<name>[^\s()]+)(?:\s+(?P<version>[^\s()]+))?(?:\s+\((?P<source>.+)\))?$")


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


def package_identity(package: dict) -> tuple[str, str, str]:
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


def spdx_id_for(package: dict) -> str:
    name, version, source = package_identity(package)
    digest = hashlib.sha256(f"{name}\0{version}\0{source}".encode()).hexdigest()[:16]
    safe_name = re.sub(r"[^A-Za-z0-9.-]", "-", name)
    return f"SPDXRef-Package-{safe_name}-{digest}"


def parse_dependency_spec(value: str) -> tuple[str, str | None, str | None]:
    if not isinstance(value, str) or not value:
        raise ValueError("Cargo.lock dependency entry must be a non-empty string")
    match = DEPENDENCY_SPEC.fullmatch(value)
    if match is None:
        raise ValueError(f"unsupported Cargo.lock dependency syntax: {value!r}")
    return match.group("name"), match.group("version"), match.group("source")


def resolve_dependency(
    packages_by_name: dict[str, list[dict]], dependency: str
) -> dict:
    name, version, source = parse_dependency_spec(dependency)
    candidates = list(packages_by_name.get(name, ()))
    if version is not None:
        candidates = [
            package for package in candidates if package_identity(package)[1] == version
        ]
    if source is not None:
        candidates = [
            package for package in candidates if package_identity(package)[2] == source
        ]
    if len(candidates) != 1:
        identities = [package_identity(package) for package in candidates]
        raise ValueError(
            f"Cargo.lock dependency {dependency!r} resolved to {len(candidates)} package records: {identities!r}"
        )
    return candidates[0]


def purl_for(package: dict) -> str:
    name, version, _source = package_identity(package)
    return f"pkg:cargo/{quote(name, safe='-._~')}@{quote(version, safe='-._~')}"


def spdx_package(package: dict) -> dict:
    name, version, source = package_identity(package)
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


def build_spdx_document(lock_data: dict, tag: str, target: str) -> dict:
    version = release_tool.check_tag(tag)
    target = release_tool.safe_component(target, "target")
    packages = lock_data.get("package")
    if not isinstance(packages, list) or not packages:
        raise ValueError("Cargo.lock contains no package records")

    validated = list(packages)
    validated.sort(key=package_identity)
    identities = [package_identity(package) for package in validated]
    if len(identities) != len(set(identities)):
        raise ValueError("Cargo.lock contains duplicate package identity records")

    packages_by_name: dict[str, list[dict]] = {}
    for package in validated:
        name, _version, _source = package_identity(package)
        packages_by_name.setdefault(name, []).append(package)

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
        }
    ]
    spdx_packages.extend(spdx_package(package) for package in validated)

    relationships: set[tuple[str, str, str]] = {
        ("SPDXRef-DOCUMENT", "DESCRIBES", root_spdx_id)
    }
    local_ids = []
    for package in validated:
        name, _version, source = package_identity(package)
        package_id = spdx_id_for(package)
        if not source and name in release_tool.LOCAL_PACKAGES:
            local_ids.append(package_id)
        dependencies = package.get("dependencies", [])
        if not isinstance(dependencies, list):
            raise ValueError(f"Cargo.lock package {name!r} has non-list dependencies")
        for dependency in dependencies:
            resolved = resolve_dependency(packages_by_name, dependency)
            relationships.add((package_id, "DEPENDS_ON", spdx_id_for(resolved)))

    if len(local_ids) != len(release_tool.LOCAL_PACKAGES):
        raise ValueError("Cargo.lock SBOM could not identify every local Kaduox package")
    for package_id in sorted(local_ids):
        relationships.add((root_spdx_id, "DEPENDS_ON", package_id))

    namespace = (
        "https://github.com/kaduoxzero/Kaduox-SSH/sbom/"
        f"{quote(tag, safe='-._~')}/{quote(target, safe='-._~')}"
    )
    return {
        "spdxVersion": SPDX_VERSION,
        "dataLicense": SPDX_DATA_LICENSE,
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"Kaduox-SSH-{version}-{target}",
        "documentNamespace": namespace,
        "creationInfo": {
            "created": NORMALIZED_CREATED,
            "creators": ["Tool: Kaduox-SSH scripts/release/sbom_tool.py"],
            "comment": "Creation time is normalized for deterministic release output; build time is carried by release provenance.",
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


def generate_sbom(tag: str, target: str, output_dir: Path) -> Path:
    version = release_tool.check_tag(tag)
    target = release_tool.safe_component(target, "target")
    document = build_spdx_document(load_lock(), tag, target)
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / f"kaduox-ssh-{version}-{target}.spdx.json"
    release_tool.write_text_safely(output, render_spdx(document))
    return output


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--tag", required=True)
    root.add_argument("--target", required=True)
    root.add_argument("--output-dir", default="dist")
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        output = generate_sbom(args.tag, args.target, Path(args.output_dir).resolve())
    except (OSError, ValueError) as exc:
        print(f"SBOM generation failed: {exc}", file=sys.stderr)
        return 2
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
