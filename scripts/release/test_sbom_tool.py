from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts.release import release_tool, sbom_tool


class SbomToolTests(unittest.TestCase):
    def test_repository_lock_generates_deterministic_spdx_graph(self) -> None:
        tag = f"v{release_tool.workspace_version()}"
        lock_data = sbom_tool.load_lock()
        created = sbom_tool.spdx_created_from_epoch(0)
        first = sbom_tool.build_spdx_document(lock_data, tag, created)
        second = sbom_tool.build_spdx_document(lock_data, tag, created)

        self.assertEqual(first, second)
        self.assertEqual(first["spdxVersion"], "SPDX-2.3")
        self.assertEqual(first["dataLicense"], "CC0-1.0")
        self.assertEqual(first["creationInfo"]["created"], "1970-01-01T00:00:00Z")
        self.assertTrue(first["documentNamespace"].endswith("/cargo-lock"))
        self.assertIn("target-pruned", first["creationInfo"]["comment"])

        package_names = {package["name"] for package in first["packages"]}
        self.assertIn("Kaduox-SSH", package_names)
        for local in release_tool.LOCAL_PACKAGES:
            self.assertIn(local, package_names)

        relationships = {
            (
                item["spdxElementId"],
                item["relationshipType"],
                item["relatedSpdxElement"],
            )
            for item in first["relationships"]
        }
        self.assertIn(
            ("SPDXRef-DOCUMENT", "DESCRIBES", "SPDXRef-Kaduox-SSH-Release"),
            relationships,
        )
        self.assertGreater(
            sum(1 for item in first["relationships"] if item["relationshipType"] == "DEPENDS_ON"),
            10,
        )
        self.assertEqual(
            sbom_tool.render_spdx(first).encode(), sbom_tool.render_spdx(second).encode()
        )

    def test_source_date_epoch_is_validated_and_normalized(self) -> None:
        self.assertEqual(sbom_tool.spdx_created_from_epoch("0"), "1970-01-01T00:00:00Z")
        self.assertEqual(sbom_tool.spdx_created_from_epoch(1), "1970-01-01T00:00:01Z")
        for value in ("not-a-number", -1):
            with self.assertRaises(ValueError, msg=str(value)):
                sbom_tool.spdx_created_from_epoch(value)

    def test_dependency_resolution_uses_version_to_disambiguate(self) -> None:
        dep_v1 = {"name": "shared", "version": "1.0.0", "source": "registry+one"}
        dep_v2 = {"name": "shared", "version": "2.0.0", "source": "registry+one"}
        packages_by_name = {"shared": [dep_v1, dep_v2]}
        self.assertIs(
            sbom_tool.resolve_dependency(packages_by_name, "shared 2.0.0"), dep_v2
        )
        with self.assertRaises(ValueError):
            sbom_tool.resolve_dependency(packages_by_name, "shared")

    def test_dependency_resolution_uses_source_to_disambiguate(self) -> None:
        registry = {
            "name": "shared",
            "version": "1.0.0",
            "source": "registry+https://example.invalid/index",
        }
        git = {
            "name": "shared",
            "version": "1.0.0",
            "source": "git+https://example.invalid/shared?rev=abc#abc",
        }
        packages_by_name = {"shared": [registry, git]}
        dependency = "shared 1.0.0 (git+https://example.invalid/shared?rev=abc#abc)"
        self.assertIs(sbom_tool.resolve_dependency(packages_by_name, dependency), git)

    def test_invalid_dependency_and_checksum_fail_closed(self) -> None:
        with self.assertRaises(ValueError):
            sbom_tool.parse_dependency_spec("")
        with self.assertRaises(ValueError):
            sbom_tool.resolve_dependency({}, "missing")
        with self.assertRaises(ValueError):
            sbom_tool.spdx_package(
                {
                    "name": "broken",
                    "version": "1.0.0",
                    "source": "registry+test",
                    "checksum": "not-a-sha256",
                }
            )

    def test_generate_sbom_writes_release_named_json(self) -> None:
        tag = f"v{release_tool.workspace_version()}"
        with tempfile.TemporaryDirectory(prefix="kaduox-sbom-test-") as temp:
            output = sbom_tool.generate_sbom(tag, Path(temp), 0)
            self.assertEqual(
                output.name,
                f"kaduox-ssh-{release_tool.workspace_version()}.spdx.json",
            )
            document = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(document["spdxVersion"], "SPDX-2.3")
            self.assertEqual(
                document["name"],
                f"Kaduox-SSH-{release_tool.workspace_version()}-Cargo.lock",
            )

    def test_render_spdx_enforces_size_budget(self) -> None:
        oversized = {"payload": "x" * sbom_tool.MAX_SBOM_BYTES}
        with self.assertRaises(ValueError):
            sbom_tool.render_spdx(oversized)


if __name__ == "__main__":
    unittest.main()
