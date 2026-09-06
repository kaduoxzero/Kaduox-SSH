from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.release import release_tool, sbom_tool


class SbomToolTests(unittest.TestCase):
    def fixture_graph(self) -> tuple[dict, dict]:
        version = release_tool.workspace_version()
        registry = "registry+https://github.com/rust-lang/crates.io-index"
        ids = {
            "cli": f"path+file:///repo/crates/kaduox-ssh-cli#kaduox-ssh-cli@{version}",
            "core": f"path+file:///repo/crates/kaduox-ssh-core#kaduox-ssh-core@{version}",
            "runtime": f"{registry}#runtime@1.0.0",
            "build": f"{registry}#build-helper@1.0.0",
            "dev": f"{registry}#dev-only@1.0.0",
            "foreign": f"{registry}#foreign-target@1.0.0",
        }
        packages = [
            {"name": "kaduox-ssh-cli", "version": version},
            {"name": "kaduox-ssh-core", "version": version},
            {
                "name": "runtime",
                "version": "1.0.0",
                "source": registry,
                "checksum": "1" * 64,
            },
            {
                "name": "build-helper",
                "version": "1.0.0",
                "source": registry,
                "checksum": "2" * 64,
            },
            {
                "name": "dev-only",
                "version": "1.0.0",
                "source": registry,
                "checksum": "3" * 64,
            },
            {
                "name": "foreign-target",
                "version": "1.0.0",
                "source": registry,
                "checksum": "4" * 64,
            },
        ]
        metadata_packages = [
            {"id": ids["cli"], "name": "kaduox-ssh-cli", "version": version, "source": None},
            {"id": ids["core"], "name": "kaduox-ssh-core", "version": version, "source": None},
            {"id": ids["runtime"], "name": "runtime", "version": "1.0.0", "source": registry},
            {"id": ids["build"], "name": "build-helper", "version": "1.0.0", "source": registry},
            {"id": ids["dev"], "name": "dev-only", "version": "1.0.0", "source": registry},
            {"id": ids["foreign"], "name": "foreign-target", "version": "1.0.0", "source": registry},
        ]
        nodes = [
            {
                "id": ids["cli"],
                "deps": [
                    {"pkg": ids["core"], "dep_kinds": [{"kind": None, "target": None}]},
                    {"pkg": ids["dev"], "dep_kinds": [{"kind": "dev", "target": None}]},
                ],
            },
            {
                "id": ids["core"],
                "deps": [
                    {"pkg": ids["runtime"], "dep_kinds": [{"kind": None, "target": None}]},
                    {"pkg": ids["build"], "dep_kinds": [{"kind": "build", "target": None}]},
                ],
            },
            {"id": ids["runtime"], "deps": []},
            {"id": ids["build"], "deps": []},
            {"id": ids["dev"], "deps": []},
            {"id": ids["foreign"], "deps": []},
        ]
        lock_data = {"version": 4, "package": packages}
        metadata = {
            "packages": metadata_packages,
            "workspace_members": [ids["cli"], ids["core"]],
            "resolve": {"nodes": nodes, "root": None},
        }
        return lock_data, metadata

    def test_target_graph_is_deterministic_and_pruned(self) -> None:
        lock_data, metadata = self.fixture_graph()
        tag = f"v{release_tool.workspace_version()}"
        target = "x86_64-unknown-linux-gnu"
        created = sbom_tool.spdx_created_from_epoch(0)
        first = sbom_tool.build_spdx_document(lock_data, metadata, tag, target, created)
        second = sbom_tool.build_spdx_document(lock_data, metadata, tag, target, created)

        self.assertEqual(first, second)
        self.assertEqual(first["spdxVersion"], "SPDX-2.3")
        self.assertEqual(first["dataLicense"], "CC0-1.0")
        self.assertEqual(first["creationInfo"]["created"], "1970-01-01T00:00:00Z")
        self.assertTrue(first["documentNamespace"].endswith(f"/{target}/cargo-metadata"))
        self.assertIn("--filter-platform", first["creationInfo"]["comment"])

        names = {package["name"] for package in first["packages"]}
        self.assertEqual(
            names,
            {"Kaduox-SSH", "kaduox-ssh-cli", "kaduox-ssh-core", "runtime", "build-helper"},
        )
        self.assertNotIn("dev-only", names)
        self.assertNotIn("foreign-target", names)

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
        self.assertTrue(any(item[1] == "DEPENDS_ON" for item in relationships))
        self.assertTrue(any(item[1] == "BUILD_DEPENDENCY_OF" for item in relationships))
        self.assertEqual(sbom_tool.render_spdx(first), sbom_tool.render_spdx(second))

    def test_release_graph_excludes_dev_only_edges(self) -> None:
        _lock_data, metadata = self.fixture_graph()
        reachable, relationships = sbom_tool.release_graph(metadata)
        packages = {package["id"]: package for package in metadata["packages"]}
        names = {packages[package_id]["name"] for package_id in reachable}
        self.assertNotIn("dev-only", names)
        self.assertNotIn("foreign-target", names)
        self.assertIn("build-helper", names)
        self.assertTrue(any(item[1] == "BUILD_DEPENDENCY_OF" for item in relationships))

    def test_unknown_dependency_kind_fails_closed(self) -> None:
        _lock_data, metadata = self.fixture_graph()
        metadata["resolve"]["nodes"][0]["deps"][0]["dep_kinds"] = [{"kind": "future-kind"}]
        with self.assertRaisesRegex(ValueError, "unsupported Cargo dependency kind"):
            sbom_tool.release_graph(metadata)

    def test_duplicate_or_unknown_workspace_members_fail_closed(self) -> None:
        _lock_data, metadata = self.fixture_graph()
        metadata["workspace_members"].append(metadata["workspace_members"][0])
        with self.assertRaisesRegex(ValueError, "duplicate workspace member"):
            sbom_tool.release_graph(metadata)

        _lock_data, metadata = self.fixture_graph()
        metadata["workspace_members"][1] = "path+file:///repo/missing#missing@1.0.0"
        with self.assertRaisesRegex(ValueError, "unknown package id"):
            sbom_tool.release_graph(metadata)

    def test_duplicate_reachable_package_identity_fails_closed(self) -> None:
        lock_data, metadata = self.fixture_graph()
        runtime = next(package for package in metadata["packages"] if package["name"] == "runtime")
        duplicate_id = runtime["id"] + "-duplicate"
        duplicate = dict(runtime)
        duplicate["id"] = duplicate_id
        metadata["packages"].append(duplicate)
        metadata["resolve"]["nodes"].append({"id": duplicate_id, "deps": []})
        core = next(node for node in metadata["resolve"]["nodes"] if "kaduox-ssh-core" in node["id"])
        core["deps"].append(
            {"pkg": duplicate_id, "dep_kinds": [{"kind": None, "target": None}]}
        )
        with self.assertRaisesRegex(ValueError, "duplicate package identities"):
            sbom_tool.build_spdx_document(
                lock_data,
                metadata,
                f"v{release_tool.workspace_version()}",
                "x86_64-unknown-linux-gnu",
                sbom_tool.spdx_created_from_epoch(0),
            )

    def test_missing_lock_package_for_resolved_target_fails_closed(self) -> None:
        lock_data, metadata = self.fixture_graph()
        lock_data["package"] = [
            package for package in lock_data["package"] if package["name"] != "runtime"
        ]
        with self.assertRaisesRegex(ValueError, "missing from Cargo.lock"):
            sbom_tool.build_spdx_document(
                lock_data,
                metadata,
                f"v{release_tool.workspace_version()}",
                "x86_64-unknown-linux-gnu",
                sbom_tool.spdx_created_from_epoch(0),
            )

    def test_cargo_metadata_command_is_locked_and_target_filtered(self) -> None:
        _lock_data, metadata = self.fixture_graph()
        completed = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=json.dumps(metadata).encode(), stderr=b""
        )
        with mock.patch.object(sbom_tool.subprocess, "run", return_value=completed) as run:
            result = sbom_tool.run_cargo_metadata("aarch64-apple-darwin")
        self.assertEqual(result, metadata)
        command = run.call_args.args[0]
        self.assertIn("--locked", command)
        self.assertEqual(command[-2:], ["--filter-platform", "aarch64-apple-darwin"])
        self.assertEqual(run.call_args.kwargs["cwd"], release_tool.ROOT)

    def test_source_date_epoch_is_validated_and_normalized(self) -> None:
        self.assertEqual(sbom_tool.spdx_created_from_epoch("0"), "1970-01-01T00:00:00Z")
        self.assertEqual(sbom_tool.spdx_created_from_epoch(1), "1970-01-01T00:00:01Z")
        for value in ("not-a-number", -1):
            with self.assertRaises(ValueError, msg=str(value)):
                sbom_tool.spdx_created_from_epoch(value)

    def test_invalid_target_and_checksum_fail_closed(self) -> None:
        with self.assertRaises(ValueError):
            sbom_tool.validate_target("not-a-release-target")
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
        lock_data, metadata = self.fixture_graph()
        tag = f"v{release_tool.workspace_version()}"
        target = "x86_64-pc-windows-msvc"
        with tempfile.TemporaryDirectory(prefix="kaduox-sbom-test-") as temp:
            with mock.patch.object(sbom_tool, "load_lock", return_value=lock_data):
                output = sbom_tool.generate_sbom(tag, target, Path(temp), 0, metadata=metadata)
            self.assertEqual(
                output.name,
                f"kaduox-ssh-{release_tool.workspace_version()}-{target}.spdx.json",
            )
            document = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                document["name"],
                f"Kaduox-SSH-{release_tool.workspace_version()}-{target}-Cargo-metadata",
            )
            root = next(package for package in document["packages"] if package["name"] == "Kaduox-SSH")
            self.assertEqual(root["comment"], f"Cargo release target: {target}")

    def test_render_spdx_enforces_size_budget(self) -> None:
        oversized = {"payload": "x" * sbom_tool.MAX_SBOM_BYTES}
        with self.assertRaises(ValueError):
            sbom_tool.render_spdx(oversized)


if __name__ == "__main__":
    unittest.main()
