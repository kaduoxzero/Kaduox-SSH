from __future__ import annotations

from collections import Counter
import re
import unittest

from scripts.release import release_tool


WORKFLOW = release_tool.ROOT / ".github" / "workflows" / "release-qualification.yml"
EXPECTED_TARGETS = Counter(
    {
        "x86_64-unknown-linux-gnu": 1,
        "x86_64-apple-darwin": 1,
        "aarch64-apple-darwin": 1,
        "x86_64-pc-windows-msvc": 1,
    }
)
TARGET_LINE = re.compile(r"^\s+target:\s+(?P<target>[^\s#]+)\s*$")


class ReleaseQualificationWorkflowPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.text = WORKFLOW.read_text(encoding="utf-8")

    def test_qualification_runs_for_published_release_and_manual_tag(self) -> None:
        self.assertIn("  release:\n    types:\n      - published\n", self.text)
        self.assertIn("  workflow_dispatch:\n    inputs:\n      tag:\n", self.text)
        self.assertIn(
            "KADUOX_RELEASE_TAG: ${{ github.event_name == 'release' && github.event.release.tag_name || inputs.tag }}",
            self.text,
        )

    def test_exact_release_target_matrix_is_preserved(self) -> None:
        targets = Counter(
            match.group("target")
            for line in self.text.splitlines()
            if (match := TARGET_LINE.match(line)) is not None
        )
        self.assertEqual(targets, EXPECTED_TARGETS)
        self.assertIn("runner: macos-15-intel", self.text)
        self.assertIn("runner: macos-15", self.text)
        self.assertIn("runner: windows-latest", self.text)
        self.assertIn("runner: ubuntu-latest", self.text)

    def test_tag_source_and_published_assets_are_used(self) -> None:
        self.assertIn("ref: ${{ env.KADUOX_RELEASE_TAG }}", self.text)
        self.assertIn("persist-credentials: false", self.text)
        self.assertIn('gh release download "$KADUOX_RELEASE_TAG"', self.text)
        self.assertIn("qualification_tool.py qualify", self.text)
        self.assertNotIn("cargo build", self.text)

    def test_linux_published_binary_runs_real_openssh_suite(self) -> None:
        self.assertIn(
            "KSSH: ${{ steps.qualify.outputs.package_dir }}/kssh", self.text
        )
        self.assertIn("run: bash scripts/ci/openssh-integration.sh", self.text)
        self.assertIn("if: matrix.target == 'x86_64-unknown-linux-gnu'", self.text)


if __name__ == "__main__":
    unittest.main()
