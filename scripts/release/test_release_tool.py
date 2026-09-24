from __future__ import annotations

import unittest

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

    def test_version_rewrite_updates_workspace_and_all_lock_records(self) -> None:
        cargo = "[workspace]\n\n[workspace.package]\nversion = \"0.13.0-rc.1\"\nedition = \"2024\"\n"
        lock = "version = 4\n\n" + "\n".join(
            f'[[package]]\nname = "{name}"\nversion = "0.13.0-rc.1"\n'
            for name in release_tool.LOCAL_PACKAGES
        )
        updated_cargo = release_tool.replace_workspace_version(
            cargo, "0.13.0-rc.1", "1.0.0"
        )
        updated_lock = release_tool.replace_lock_versions(
            lock, "0.13.0-rc.1", "1.0.0"
        )
        self.assertIn('version = "1.0.0"', updated_cargo)
        self.assertEqual(
            updated_lock.count('version = "1.0.0"'), len(release_tool.LOCAL_PACKAGES)
        )
        self.assertNotIn("0.13.0-rc.1", updated_cargo)
        self.assertNotIn("0.13.0-rc.1", updated_lock)

    def test_version_rewrite_fails_before_mutation_on_unexpected_lock(self) -> None:
        lock = "\n".join(
            f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
            for name, version in zip(
                release_tool.LOCAL_PACKAGES,
                ["0.13.0-rc.1"] * (len(release_tool.LOCAL_PACKAGES) - 1) + ["0.12.0"],
            )
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


if __name__ == "__main__":
    unittest.main()
