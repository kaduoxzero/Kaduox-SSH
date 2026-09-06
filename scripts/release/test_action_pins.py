from __future__ import annotations

from collections import Counter
import re
import unittest

from scripts.release import release_tool


WORKFLOW_FILES = {
    "release": release_tool.ROOT / ".github" / "workflows" / "release.yml",
    "qualification": release_tool.ROOT / ".github" / "workflows" / "release-qualification.yml",
    "ci": release_tool.ROOT / ".github" / "workflows" / "ci.yml",
    "quality": release_tool.ROOT / ".github" / "workflows" / "quality.yml",
    "openssh": release_tool.ROOT / ".github" / "workflows" / "integration-openssh.yml",
}
USES_LINE = re.compile(
    r"^\s*(?:-\s+)?uses:\s+(?P<spec>[^\s#]+)(?:\s+#\s*(?P<comment>.+))?\s*$"
)
FULL_COMMIT_SHA = re.compile(r"^[0-9a-f]{40}$")

CHECKOUT_V4 = "11d5960a326750d5838078e36cf38b85af677262"
CHECKOUT_V7 = "3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_PYTHON_V7 = "5fda3b95a4ea91299a34e894583c3862153e4b97"
RUST_TOOLCHAIN_ACTION = "6bed0761d98439e5a578e2877258200ad565ba87"
RUST_CACHE_V2 = "6323deb102c322ba6fcbdcafc7e3dddab59af2b6"
ACTIONS_CACHE_V5 = "9255dc7a253b0ccc959486e2bca901246202afeb"
UPLOAD_ARTIFACT_V4 = "ea165f8d65b6e75b540449e92b4886f43607fa02"
DOWNLOAD_ARTIFACT_V4 = "d3f86a106a0bac45b974a628896c90dbdf5c8093"
ATTEST_V4 = "1e69f48acb82d1966a394da916b4c1698aa569d6"


def spec(action: str, commit: str) -> str:
    return f"{action}@{commit}"


EXPECTED_WORKFLOW_SPECS: dict[str, Counter[str]] = {
    "release": Counter(
        {
            spec("actions/checkout", CHECKOUT_V7): 2,
            spec("actions/setup-python", SETUP_PYTHON_V7): 2,
            spec("dtolnay/rust-toolchain", RUST_TOOLCHAIN_ACTION): 1,
            spec("Swatinem/rust-cache", RUST_CACHE_V2): 1,
            spec("actions/upload-artifact", UPLOAD_ARTIFACT_V4): 1,
            spec("actions/download-artifact", DOWNLOAD_ARTIFACT_V4): 2,
            spec("actions/attest", ATTEST_V4): 2,
        }
    ),
    "qualification": Counter(
        {
            spec("actions/checkout", CHECKOUT_V7): 1,
            spec("actions/setup-python", SETUP_PYTHON_V7): 1,
        }
    ),
    "ci": Counter(
        {
            spec("actions/checkout", CHECKOUT_V4): 2,
            spec("dtolnay/rust-toolchain", RUST_TOOLCHAIN_ACTION): 2,
            spec("Swatinem/rust-cache", RUST_CACHE_V2): 2,
        }
    ),
    "quality": Counter(
        {
            spec("actions/checkout", CHECKOUT_V4): 2,
            spec("dtolnay/rust-toolchain", RUST_TOOLCHAIN_ACTION): 2,
            spec("Swatinem/rust-cache", RUST_CACHE_V2): 1,
            spec("actions/cache", ACTIONS_CACHE_V5): 1,
            spec("actions/checkout", CHECKOUT_V7): 1,
            spec("actions/setup-python", SETUP_PYTHON_V7): 1,
        }
    ),
    "openssh": Counter(
        {
            spec("actions/checkout", CHECKOUT_V4): 1,
            spec("dtolnay/rust-toolchain", RUST_TOOLCHAIN_ACTION): 1,
            spec("Swatinem/rust-cache", RUST_CACHE_V2): 1,
        }
    ),
}


def external_action_uses(text: str) -> list[tuple[int, str, str | None]]:
    found: list[tuple[int, str, str | None]] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        match = USES_LINE.match(line)
        if match is None:
            continue
        action_spec = match.group("spec")
        if action_spec.startswith("./"):
            continue
        found.append((line_number, action_spec, match.group("comment")))
    return found


def validate_workflow_action_pins(
    text: str, expected: Counter[str], workflow_label: str
) -> list[str]:
    uses = external_action_uses(text)
    if not uses:
        raise ValueError(f"{workflow_label} workflow contains no external action uses")

    actual: Counter[str] = Counter()
    ordered_specs: list[str] = []
    for line_number, action_spec, comment in uses:
        if action_spec.startswith("docker://"):
            raise ValueError(
                f"{workflow_label} workflow line {line_number} uses an unpinned container action: {action_spec}"
            )
        if action_spec.count("@") != 1:
            raise ValueError(
                f"{workflow_label} workflow line {line_number} must use owner/repository@<40-char-sha>: {action_spec}"
            )
        action, ref = action_spec.rsplit("@", 1)
        if not action or "/" not in action:
            raise ValueError(
                f"{workflow_label} workflow line {line_number} has an invalid external action reference: {action_spec}"
            )
        if FULL_COMMIT_SHA.fullmatch(ref) is None:
            raise ValueError(
                f"{workflow_label} workflow line {line_number} is not pinned to an immutable 40-character commit SHA: {action_spec}"
            )
        if not comment or not comment.strip():
            raise ValueError(
                f"{workflow_label} workflow line {line_number} must retain a human-readable version/ref comment for {action}"
            )
        actual[action_spec] += 1
        ordered_specs.append(action_spec)

    if actual != expected:
        missing = sorted((expected - actual).elements())
        unexpected = sorted((actual - expected).elements())
        raise ValueError(
            f"{workflow_label} workflow external action set changed; missing={missing!r}, unexpected={unexpected!r}. "
            "Review the upstream commit and update the explicit workflow pin policy in the same change."
        )
    return ordered_specs


class WorkflowActionPinTests(unittest.TestCase):
    def test_repository_workflows_match_exact_approved_action_commits(self) -> None:
        self.assertEqual(set(WORKFLOW_FILES), set(EXPECTED_WORKFLOW_SPECS))
        for label, path in WORKFLOW_FILES.items():
            with self.subTest(workflow=label):
                text = path.read_text(encoding="utf-8")
                specs = validate_workflow_action_pins(
                    text, EXPECTED_WORKFLOW_SPECS[label], label
                )
                self.assertEqual(Counter(specs), EXPECTED_WORKFLOW_SPECS[label])

    def test_mutable_tags_branches_and_short_shas_fail_closed(self) -> None:
        expected = Counter({spec("actions/checkout", "a" * 40): 1})
        for action_spec in (
            "actions/checkout@v7",
            "actions/checkout@stable",
            "actions/checkout@3d3c42e",
        ):
            with self.subTest(spec=action_spec), self.assertRaises(ValueError):
                validate_workflow_action_pins(
                    f"steps:\n  - uses: {action_spec} # readable\n",
                    expected,
                    "test",
                )

    def test_changed_or_unapproved_full_sha_fails_closed(self) -> None:
        approved_spec = spec("actions/example", "a" * 40)
        expected = Counter({approved_spec: 1})
        for action_spec in (
            spec("actions/example", "b" * 40),
            spec("other/example", "a" * 40),
        ):
            with self.subTest(spec=action_spec), self.assertRaises(ValueError):
                validate_workflow_action_pins(
                    f"steps:\n  - uses: {action_spec} # v1\n",
                    expected,
                    "test",
                )

    def test_approved_full_sha_requires_human_readable_comment(self) -> None:
        action_spec = spec("actions/example", "a" * 40)
        expected = Counter({action_spec: 1})
        with self.assertRaises(ValueError):
            validate_workflow_action_pins(
                f"steps:\n  - uses: {action_spec}\n", expected, "test"
            )
        self.assertEqual(
            validate_workflow_action_pins(
                f"steps:\n  - uses: {action_spec} # v1.2.3\n",
                expected,
                "test",
            ),
            [action_spec],
        )

    def test_exact_occurrence_counts_are_enforced(self) -> None:
        action_spec = spec("actions/example", "a" * 40)
        expected = Counter({action_spec: 1})
        text = (
            "steps:\n"
            f"  - uses: {action_spec} # v1\n"
            f"  - uses: {action_spec} # v1\n"
        )
        with self.assertRaises(ValueError):
            validate_workflow_action_pins(text, expected, "test")

    def test_job_level_reusable_workflow_uses_are_checked(self) -> None:
        action_spec = spec("example/reusable/.github/workflows/build.yml", "a" * 40)
        expected = Counter({action_spec: 1})
        text = f"jobs:\n  build:\n    uses: {action_spec} # v1\n"
        self.assertEqual(
            validate_workflow_action_pins(text, expected, "test"),
            [action_spec],
        )
        with self.assertRaises(ValueError):
            validate_workflow_action_pins(
                "jobs:\n  build:\n    uses: example/reusable/.github/workflows/build.yml@v1 # v1\n",
                expected,
                "test",
            )

    def test_local_actions_do_not_require_git_commit_refs(self) -> None:
        action_spec = spec("actions/example", "b" * 40)
        expected = Counter({action_spec: 1})
        text = (
            "steps:\n"
            "  - uses: ./actions/local\n"
            f"  - uses: {action_spec} # v1\n"
        )
        self.assertEqual(
            validate_workflow_action_pins(text, expected, "test"),
            [action_spec],
        )


if __name__ == "__main__":
    unittest.main()
