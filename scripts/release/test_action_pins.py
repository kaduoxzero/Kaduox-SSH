from __future__ import annotations

import re
import unittest
from pathlib import Path

from scripts.release import release_tool


RELEASE_WORKFLOW = release_tool.ROOT / ".github" / "workflows" / "release.yml"
USES_LINE = re.compile(
    r"^\s*-\s+uses:\s+(?P<spec>[^\s#]+)(?:\s+#\s*(?P<comment>.+))?\s*$"
)
FULL_COMMIT_SHA = re.compile(r"^[0-9a-f]{40}$")


def external_action_uses(text: str) -> list[tuple[int, str, str | None]]:
    found: list[tuple[int, str, str | None]] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        match = USES_LINE.match(line)
        if match is None:
            continue
        spec = match.group("spec")
        if spec.startswith("./"):
            continue
        found.append((line_number, spec, match.group("comment")))
    return found


def validate_release_action_pins(text: str) -> list[str]:
    uses = external_action_uses(text)
    if not uses:
        raise ValueError("release workflow contains no external action uses")

    specs: list[str] = []
    for line_number, spec, comment in uses:
        if spec.startswith("docker://"):
            raise ValueError(
                f"release workflow line {line_number} uses an unpinned container action: {spec}"
            )
        if spec.count("@") != 1:
            raise ValueError(
                f"release workflow line {line_number} must use owner/repository@<40-char-sha>: {spec}"
            )
        action, ref = spec.rsplit("@", 1)
        if not action or "/" not in action:
            raise ValueError(
                f"release workflow line {line_number} has an invalid external action reference: {spec}"
            )
        if FULL_COMMIT_SHA.fullmatch(ref) is None:
            raise ValueError(
                f"release workflow line {line_number} is not pinned to an immutable 40-character commit SHA: {spec}"
            )
        if not comment or not comment.strip():
            raise ValueError(
                f"release workflow line {line_number} must retain a human-readable version/ref comment for {action}"
            )
        specs.append(spec)
    return specs


class ReleaseActionPinTests(unittest.TestCase):
    def test_repository_release_workflow_uses_only_full_sha_pins(self) -> None:
        text = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        specs = validate_release_action_pins(text)
        self.assertGreaterEqual(len(specs), 10)
        self.assertTrue(all(FULL_COMMIT_SHA.fullmatch(spec.rsplit("@", 1)[1]) for spec in specs))

    def test_mutable_tags_branches_and_short_shas_fail_closed(self) -> None:
        for spec in (
            "actions/checkout@v7",
            "dtolnay/rust-toolchain@stable",
            "actions/upload-artifact@ea165f8",
        ):
            with self.subTest(spec=spec), self.assertRaises(ValueError):
                validate_release_action_pins(f"steps:\n  - uses: {spec} # readable\n")

    def test_full_sha_requires_human_readable_comment(self) -> None:
        sha = "a" * 40
        with self.assertRaises(ValueError):
            validate_release_action_pins(f"steps:\n  - uses: actions/example@{sha}\n")
        self.assertEqual(
            validate_release_action_pins(
                f"steps:\n  - uses: actions/example@{sha} # v1.2.3\n"
            ),
            [f"actions/example@{sha}"],
        )

    def test_local_actions_do_not_require_git_commit_refs(self) -> None:
        sha = "b" * 40
        text = (
            "steps:\n"
            "  - uses: ./actions/local\n"
            f"  - uses: actions/example@{sha} # v1\n"
        )
        self.assertEqual(validate_release_action_pins(text), [f"actions/example@{sha}"])


if __name__ == "__main__":
    unittest.main()
