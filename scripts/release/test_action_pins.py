from __future__ import annotations

import re
import unittest

from scripts.release import release_tool


RELEASE_WORKFLOW = release_tool.ROOT / ".github" / "workflows" / "release.yml"
USES_LINE = re.compile(
    r"^\s*-\s+uses:\s+(?P<spec>[^\s#]+)(?:\s+#\s*(?P<comment>.+))?\s*$"
)
FULL_COMMIT_SHA = re.compile(r"^[0-9a-f]{40}$")
APPROVED_ACTION_PINS = {
    "actions/checkout": "3d3c42e5aac5ba805825da76410c181273ba90b1",
    "actions/setup-python": "5fda3b95a4ea91299a34e894583c3862153e4b97",
    "dtolnay/rust-toolchain": "6bed0761d98439e5a578e2877258200ad565ba87",
    "Swatinem/rust-cache": "6323deb102c322ba6fcbdcafc7e3dddab59af2b6",
    "actions/upload-artifact": "ea165f8d65b6e75b540449e92b4886f43607fa02",
    "actions/download-artifact": "d3f86a106a0bac45b974a628896c90dbdf5c8093",
    "actions/attest": "1e69f48acb82d1966a394da916b4c1698aa569d6",
}


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


def validate_release_action_pins(
    text: str, approved: dict[str, str] | None = None
) -> list[str]:
    approved = APPROVED_ACTION_PINS if approved is None else approved
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
        expected = approved.get(action)
        if expected is None:
            raise ValueError(
                f"release workflow line {line_number} uses unapproved external action {action}; add its reviewed commit SHA to the release pin policy"
            )
        if ref != expected:
            raise ValueError(
                f"release workflow line {line_number} pins {action} to {ref}, but release policy approves {expected}"
            )
        if not comment or not comment.strip():
            raise ValueError(
                f"release workflow line {line_number} must retain a human-readable version/ref comment for {action}"
            )
        specs.append(spec)
    return specs


class ReleaseActionPinTests(unittest.TestCase):
    def test_repository_release_workflow_matches_approved_action_commits(self) -> None:
        text = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        specs = validate_release_action_pins(text)
        self.assertGreaterEqual(len(specs), 10)
        self.assertEqual(
            {spec.rsplit("@", 1)[0] for spec in specs},
            set(APPROVED_ACTION_PINS),
        )

    def test_mutable_tags_branches_and_short_shas_fail_closed(self) -> None:
        approved = {"actions/checkout": APPROVED_ACTION_PINS["actions/checkout"]}
        for spec in (
            "actions/checkout@v7",
            "actions/checkout@stable",
            "actions/checkout@3d3c42e",
        ):
            with self.subTest(spec=spec), self.assertRaises(ValueError):
                validate_release_action_pins(
                    f"steps:\n  - uses: {spec} # readable\n", approved
                )

    def test_unapproved_or_changed_full_sha_fails_closed(self) -> None:
        approved = {"actions/example": "a" * 40}
        for spec in (
            f"actions/example@{'b' * 40}",
            f"other/example@{'a' * 40}",
        ):
            with self.subTest(spec=spec), self.assertRaises(ValueError):
                validate_release_action_pins(
                    f"steps:\n  - uses: {spec} # v1\n", approved
                )

    def test_approved_full_sha_requires_human_readable_comment(self) -> None:
        sha = "a" * 40
        approved = {"actions/example": sha}
        with self.assertRaises(ValueError):
            validate_release_action_pins(
                f"steps:\n  - uses: actions/example@{sha}\n", approved
            )
        self.assertEqual(
            validate_release_action_pins(
                f"steps:\n  - uses: actions/example@{sha} # v1.2.3\n", approved
            ),
            [f"actions/example@{sha}"],
        )

    def test_local_actions_do_not_require_git_commit_refs(self) -> None:
        sha = "b" * 40
        approved = {"actions/example": sha}
        text = (
            "steps:\n"
            "  - uses: ./actions/local\n"
            f"  - uses: actions/example@{sha} # v1\n"
        )
        self.assertEqual(
            validate_release_action_pins(text, approved),
            [f"actions/example@{sha}"],
        )


if __name__ == "__main__":
    unittest.main()
