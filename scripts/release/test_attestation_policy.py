from __future__ import annotations

from collections import Counter
import re
import unittest

from scripts.release import release_tool


WORKFLOW = release_tool.ROOT / ".github" / "workflows" / "release.yml"
ATTEST_SHA = "1e69f48acb82d1966a394da916b4c1698aa569d6"
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


class ReleaseAttestationPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.text = WORKFLOW.read_text(encoding="utf-8")
        self.attest_block = self.text.split("\n  attest:\n", 1)[1].split("\n  publish:\n", 1)[0]
        self.publish_block = self.text.split("\n  publish:\n", 1)[1]
        self.build_block = self.text.split("\n  build:\n", 1)[1].split("\n  attest:\n", 1)[0]

    def test_stable_release_attestation_is_mandatory(self) -> None:
        self.assertIn(
            "if: ${{ !contains(github.ref_name, '-') || vars.KADUOX_ENABLE_GITHUB_ATTESTATIONS == 'true' }}",
            self.attest_block,
        )
        self.assertIn(
            "if: ${{ always() && needs.preflight.result == 'success' && needs.build.result == 'success' && (needs.attest.result == 'success' || (contains(github.ref_name, '-') && needs.attest.result == 'skipped')) }}",
            self.publish_block,
        )
        self.assertNotIn("needs.attest.result == 'skipped') }}", self.publish_block)

    def test_attestation_job_has_exact_release_target_matrix(self) -> None:
        matrix = re.search(
            r"strategy:\n\s+fail-fast: false\n\s+matrix:\n\s+target:\n(?P<body>(?:\s+- [^\n]+\n)+)",
            self.attest_block,
        )
        self.assertIsNotNone(matrix)
        targets = Counter(
            line.strip()[2:]
            for line in matrix.group("body").splitlines()
            if line.strip().startswith("- ")
        )
        self.assertEqual(targets, Counter({target: 1 for target in TARGETS}))

    def test_target_provenance_and_sbom_binding_are_both_required(self) -> None:
        action = f"actions/attest@{ATTEST_SHA} # v4"
        self.assertEqual(self.attest_block.count(action), 2)
        self.assertIn(
            "subject-path: |\n            ${{ steps.subject.outputs.archive }}\n            ${{ steps.subject.outputs.sbom }}",
            self.attest_block,
        )
        self.assertIn(
            "subject-path: ${{ steps.subject.outputs.archive }}\n          sbom-path: ${{ steps.subject.outputs.sbom }}",
            self.attest_block,
        )
        self.assertNotIn("create-storage-record:", self.attest_block)

    def test_attestation_permissions_are_minimal_and_oidc_enabled(self) -> None:
        self.assertIn("contents: read", self.attest_block)
        self.assertIn("id-token: write", self.attest_block)
        self.assertIn("attestations: write", self.attest_block)
        self.assertNotIn("contents: write", self.attest_block)

    def test_each_build_generates_target_specific_sbom(self) -> None:
        self.assertNotIn("\n  sbom:\n", self.text)
        self.assertIn("Generate target-specific SPDX SBOM", self.build_block)
        self.assertIn("python scripts/release/sbom_tool.py", self.build_block)
        self.assertIn('--target "${{ matrix.target }}"', self.build_block)
        self.assertIn("--source-date-epoch", self.build_block)

    def test_publish_requires_exact_four_archive_four_sbom_set(self) -> None:
        self.assertIn("${#archives[@]} -ne 4", self.publish_block)
        self.assertIn("${#sboms[@]} -ne 4", self.publish_block)
        self.assertIn("${#unexpected[@]} -ne 0", self.publish_block)
        self.assertIn('test "$(wc -l < dist/SHA256SUMS)" -eq 8', self.publish_block)
        for target in TARGETS:
            self.assertIn(target, self.publish_block)


if __name__ == "__main__":
    unittest.main()
