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
        first = sbom_tool.build_spdx_document(
            lock_data, tag, "x86_64-unknown-linux-gnu"
        )
        second = sbom_tool.build_spdx_document(
            lock_data, tag, "x86_64-unknown-linux-gnu"
        )

        self.assertEqual(first, second)
        self.assertEqual(first["spdxVersion"], "SPDX-2.3")
        self.assertEqual(first["dataLicense"], "CC0-1.0")
        self.assertEqual(
            first["creationInfo"]["created"], sbom_tool.NORMALIZED_CREATED
        )
        self.assertIn("x86_64-unknown-linux-gnu", first["documentNamespace"])

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

    def test_generate_sbom_writes_target_named_json(self) -> None:
        tag = f"v{release_tool.workspace_version()}"
        with tempfile.TemporaryDirectory(prefix="kaduox-sbom-test-") as temp:
            output = sbom_tool.generate_sbom(
                tag, "x86_64-unknown-linux-gnu", Path(temp)
            )
            self.assertEqual(
                output.name,
                f"kaduox-ssh-{release_tool.workspace_version()}-x86_64-unknown-linux-gnu.spdx.json",
            )
            document = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(document["spdxVersion"], "SPDX-2.3")
            self.assertEqual(document["name"], f"Kaduox-SSH-{release_tool.workspace_version()}-x86_64-unknown-linux-gnu")

    def test_render_spdx_enforces_size_budget(self) -> None:
        oversized = {"payload": "x" * sbom_tool.MAX_SBOM_BYTES}
        with self.assertRaises(ValueError):
            sbom_tool.render_spdx(oversized)


if __name__ == "__main__":
    unittest.main()
