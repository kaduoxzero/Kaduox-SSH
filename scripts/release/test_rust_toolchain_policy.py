from __future__ import annotations

from collections import Counter
import re
import unittest

from scripts.release import release_tool


PINNED_RUST = "1.98.1"
MSRV_RUST = "1.85.0"
WORKFLOW_FILES = {
    "release": release_tool.ROOT / ".github" / "workflows" / "release.yml",
    "ci": release_tool.ROOT / ".github" / "workflows" / "ci.yml",
    "quality": release_tool.ROOT / ".github" / "workflows" / "quality.yml",
    "openssh": release_tool.ROOT / ".github" / "workflows" / "integration-openssh.yml",
}
EXPECTED_TOOLCHAINS = {
    "release": Counter({PINNED_RUST: 1}),
    "ci": Counter({PINNED_RUST: 1, MSRV_RUST: 1}),
    "quality": Counter({PINNED_RUST: 2}),
    "openssh": Counter({PINNED_RUST: 1}),
}
RUST_ACTION = re.compile(r"^(?P<indent>\s*)-\s+uses:\s+dtolnay/rust-toolchain@")
TOOLCHAIN_INPUT = re.compile(r"^\s+toolchain:\s*[\"']?(?P<version>[^\s\"']+)")


def rust_toolchain_inputs(text: str) -> list[str]:
    lines = text.splitlines()
    versions: list[str] = []
    for index, line in enumerate(lines):
        match = RUST_ACTION.match(line)
        if match is None:
            continue
        base_indent = len(match.group("indent"))
        version: str | None = None
        for following in lines[index + 1 :]:
            stripped = following.strip()
            if not stripped:
                continue
            indent = len(following) - len(following.lstrip())
            if indent <= base_indent and stripped.startswith("-"):
                break
            toolchain = TOOLCHAIN_INPUT.match(following)
            if toolchain is not None:
                version = toolchain.group("version")
                break
        if version is None:
            raise ValueError("dtolnay/rust-toolchain use is missing an explicit toolchain input")
        versions.append(version)
    return versions


def validate_workflow_toolchains(text: str, expected: Counter[str], label: str) -> None:
    versions = Counter(rust_toolchain_inputs(text))
    if versions != expected:
        raise ValueError(
            f"{label} workflow Rust toolchain set changed: expected={expected!r}, actual={versions!r}"
        )
    lowered = text.lower()
    if "cargo +stable" in lowered or "toolchain: stable" in lowered:
        raise ValueError(f"{label} workflow contains a moving Rust stable toolchain reference")


class RustToolchainPolicyTests(unittest.TestCase):
    def test_repository_workflows_use_exact_toolchains(self) -> None:
        self.assertEqual(set(WORKFLOW_FILES), set(EXPECTED_TOOLCHAINS))
        for label, path in WORKFLOW_FILES.items():
            with self.subTest(workflow=label):
                validate_workflow_toolchains(
                    path.read_text(encoding="utf-8"), EXPECTED_TOOLCHAINS[label], label
                )

    def test_missing_or_moving_toolchain_fails_closed(self) -> None:
        for text in (
            "steps:\n  - uses: dtolnay/rust-toolchain@" + "a" * 40 + "\n",
            "steps:\n  - uses: dtolnay/rust-toolchain@" + "a" * 40 + "\n    with:\n      toolchain: stable\n",
        ):
            with self.subTest(text=text), self.assertRaises(ValueError):
                validate_workflow_toolchains(text, Counter({PINNED_RUST: 1}), "test")

    def test_quality_audit_is_pinned_to_same_compiler_and_scanner(self) -> None:
        text = WORKFLOW_FILES["quality"].read_text(encoding="utf-8")
        self.assertNotIn("actions-rust-lang/audit@", text)
        self.assertIn(
            "cargo +1.98.1 install cargo-audit --version 0.22.0 --locked --no-default-features",
            text,
        )
        self.assertIn("cargo +1.98.1 audit --file Cargo.lock", text)

    def test_msrv_remains_distinct_from_release_toolchain(self) -> None:
        self.assertNotEqual(PINNED_RUST, MSRV_RUST)
        self.assertEqual(MSRV_RUST, "1.85.0")


if __name__ == "__main__":
    unittest.main()
