from __future__ import annotations

import json
import os
import shutil
import stat
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

from scripts.release import release_tool


class ReleaseToolTests(unittest.TestCase):
    def test_repository_declares_exact_release_binary_suite(self) -> None:
        version = release_tool.validate_repository()
        self.assertTrue(version)
        self.assertEqual(release_tool.declared_binaries(), release_tool.EXPECTED_BINARIES)
        self.assertEqual(
            release_tool.lock_local_versions(),
            {name: version for name in release_tool.LOCAL_PACKAGES},
        )

    def test_tag_must_match_workspace_version_exactly(self) -> None:
        version = release_tool.workspace_version()
        self.assertEqual(release_tool.check_tag(f"v{version}"), version)
        with self.assertRaises(ValueError):
            release_tool.check_tag(version)
        with self.assertRaises(ValueError):
            release_tool.check_tag(f"v{version}-other")

    def test_semver_policy_rejects_ambiguous_versions(self) -> None:
        for value in ("", "1", "1.0", "01.0.0", "1.0.0-", "v1.0.0", "1.0.0 bad"):
            self.assertIsNone(release_tool.SEMVER.fullmatch(value), value)
        for value in ("0.13.0-rc.1", "1.0.0", "1.0.0+build.7"):
            self.assertIsNotNone(release_tool.SEMVER.fullmatch(value), value)

    def test_version_rewrite_updates_workspace_and_both_lock_records(self) -> None:
        cargo = "[workspace]\n\n[workspace.package]\nversion = \"0.13.0-rc.1\"\nedition = \"2024\"\n"
        lock = (
            "version = 4\n\n"
            "[[package]]\nname = \"kaduox-ssh-cli\"\nversion = \"0.13.0-rc.1\"\n\n"
            "[[package]]\nname = \"kaduox-ssh-core\"\nversion = \"0.13.0-rc.1\"\n"
        )
        updated_cargo = release_tool.replace_workspace_version(
            cargo, "0.13.0-rc.1", "1.0.0"
        )
        updated_lock = release_tool.replace_lock_versions(
            lock, "0.13.0-rc.1", "1.0.0"
        )
        self.assertIn('version = "1.0.0"', updated_cargo)
        self.assertEqual(updated_lock.count('version = "1.0.0"'), 2)
        self.assertNotIn("0.13.0-rc.1", updated_cargo)
        self.assertNotIn("0.13.0-rc.1", updated_lock)

    def test_version_rewrite_fails_before_mutation_on_unexpected_lock(self) -> None:
        lock = (
            "[[package]]\nname = \"kaduox-ssh-cli\"\nversion = \"0.13.0-rc.1\"\n\n"
            "[[package]]\nname = \"kaduox-ssh-core\"\nversion = \"0.12.0\"\n"
        )
        with self.assertRaises(ValueError):
            release_tool.replace_lock_versions(lock, "0.13.0-rc.1", "1.0.0")

    def test_path_components_fail_closed(self) -> None:
        for value in ("", ".", "..", "a/b", "a\\b", "line\nname"):
            with self.assertRaises(ValueError, msg=value):
                release_tool.safe_component(value, "test")
        self.assertEqual(
            release_tool.safe_component("x86_64-unknown-linux-gnu", "test"),
            "x86_64-unknown-linux-gnu",
        )

    def test_tar_package_contains_all_binaries_docs_and_manifest(self) -> None:
        self._exercise_package("test-release-linux", "tar.gz", "")

    def test_zip_package_contains_all_binaries_docs_and_manifest(self) -> None:
        self._exercise_package("test-release-windows", "zip", ".exe")

    def test_archive_metadata_is_normalized_and_repeatable(self) -> None:
        for archive_format, suffix, target in (
            ("tar.gz", "", "test-repeat-linux"),
            ("zip", ".exe", "test-repeat-windows"),
        ):
            release_dir = release_tool.ROOT / "target" / target / "release"
            shutil.rmtree(release_tool.ROOT / "target" / target, ignore_errors=True)
            release_dir.mkdir(parents=True)
            try:
                for index, binary in enumerate(release_tool.EXPECTED_BINARIES):
                    path = release_dir / f"{binary}{suffix}"
                    path.write_bytes(f"repeat:{binary}".encode())
                    os.utime(path, (1_000 + index, 2_000 + index))
                version = release_tool.workspace_version()
                with tempfile.TemporaryDirectory(prefix="kaduox-repeat-a-") as first_dir, tempfile.TemporaryDirectory(prefix="kaduox-repeat-b-") as second_dir:
                    first = release_tool.package_release(
                        f"v{version}", target, archive_format, suffix, Path(first_dir)
                    )
                    for index, binary in enumerate(release_tool.EXPECTED_BINARIES):
                        path = release_dir / f"{binary}{suffix}"
                        os.utime(path, (9_000 + index, 10_000 + index))
                    second = release_tool.package_release(
                        f"v{version}", target, archive_format, suffix, Path(second_dir)
                    )
                    self.assertEqual(first.read_bytes(), second.read_bytes())
            finally:
                shutil.rmtree(release_tool.ROOT / "target" / target, ignore_errors=True)

    def _exercise_package(self, target: str, archive_format: str, exe_suffix: str) -> None:
        release_dir = release_tool.ROOT / "target" / target / "release"
        shutil.rmtree(release_tool.ROOT / "target" / target, ignore_errors=True)
        release_dir.mkdir(parents=True)
        try:
            for binary in release_tool.EXPECTED_BINARIES:
                path = release_dir / f"{binary}{exe_suffix}"
                path.write_bytes(f"fixture:{binary}".encode())

            version = release_tool.workspace_version()
            with tempfile.TemporaryDirectory(prefix="kaduox-release-test-") as temp:
                archive = release_tool.package_release(
                    tag=f"v{version}",
                    target=target,
                    archive_format=archive_format,
                    exe_suffix=exe_suffix,
                    output_dir=Path(temp),
                )
                self.assertTrue(archive.is_file())
                names, manifest = self._read_archive(archive, archive_format)
                prefix = f"kaduox-ssh-{version}-{target}/"
                for binary in release_tool.EXPECTED_BINARIES:
                    self.assertIn(f"{prefix}{binary}{exe_suffix}", names)
                for document in ("README.md", "README.zh-CN.md", "LICENSE", "manifest.json"):
                    self.assertIn(f"{prefix}{document}", names)
                self.assertEqual(manifest["schema"], 1)
                self.assertEqual(manifest["version"], version)
                self.assertEqual(manifest["target"], target)
                self.assertEqual(
                    [entry["name"] for entry in manifest["binaries"]],
                    list(release_tool.EXPECTED_BINARIES),
                )
                self.assertTrue(all(len(entry["sha256"]) == 64 for entry in manifest["binaries"]))
                self._assert_archive_modes(archive, archive_format, prefix, exe_suffix)
        finally:
            shutil.rmtree(release_tool.ROOT / "target" / target, ignore_errors=True)

    def _assert_archive_modes(
        self, archive: Path, archive_format: str, prefix: str, exe_suffix: str
    ) -> None:
        expected = f"{prefix}kssh{exe_suffix}"
        if archive_format == "tar.gz":
            with tarfile.open(archive, "r:gz") as handle:
                member = handle.getmember(expected)
                self.assertEqual(member.mtime, 0)
                self.assertEqual(member.uid, 0)
                self.assertEqual(member.gid, 0)
                self.assertEqual(member.mode & 0o777, 0o755)
            return
        with zipfile.ZipFile(archive, "r") as handle:
            info = handle.getinfo(expected)
            self.assertEqual(info.date_time, release_tool.ZIP_FILE_TIME)
            mode = (info.external_attr >> 16) & 0o777
            self.assertEqual(mode, 0o755)
            self.assertTrue((info.external_attr >> 16) & stat.S_IFREG)

    def _read_archive(self, archive: Path, archive_format: str) -> tuple[set[str], dict]:
        if archive_format == "tar.gz":
            with tarfile.open(archive, "r:gz") as handle:
                names = set(handle.getnames())
                manifest_name = next(name for name in names if name.endswith("/manifest.json"))
                member = handle.extractfile(manifest_name)
                assert member is not None
                manifest = json.loads(member.read().decode("utf-8"))
                return names, manifest
        with zipfile.ZipFile(archive, "r") as handle:
            names = set(handle.namelist())
            manifest_name = next(name for name in names if name.endswith("/manifest.json"))
            manifest = json.loads(handle.read(manifest_name).decode("utf-8"))
            return names, manifest


if __name__ == "__main__":
    unittest.main()
