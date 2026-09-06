from __future__ import annotations

import io
import json
import os
import stat
import tarfile
import tempfile
import unittest
from pathlib import Path
from urllib.parse import quote

from scripts.release import qualification_tool, release_tool


class ReleaseQualificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.version = release_tool.workspace_version()
        self.tag = f"v{self.version}"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def _write_package(self, target: str, archive_format: str, exe_suffix: str) -> Path:
        name = qualification_tool.package_name(self.version, target)
        package = self.root / f"stage-{target}" / name
        package.mkdir(parents=True)
        entries = []
        for binary in release_tool.EXPECTED_BINARIES:
            filename = f"{binary}{exe_suffix}"
            path = package / filename
            path.write_text(
                f"#!/bin/sh\nprintf '%s\\n' '{binary} {self.version}'\n",
                encoding="utf-8",
            )
            os.chmod(path, 0o755)
            entries.append(
                {
                    "name": binary,
                    "file": filename,
                    "sha256": qualification_tool.sha256(path),
                    "bytes": path.stat().st_size,
                }
            )
        for document in ("README.md", "README.zh-CN.md", "LICENSE"):
            path = package / document
            path.write_text(f"fixture {document}\n", encoding="utf-8")
            os.chmod(path, 0o644)
        manifest = {
            "schema": 1,
            "project": "Kaduox-SSH",
            "tag": self.tag,
            "version": self.version,
            "target": target,
            "binaries": entries,
        }
        manifest_path = package / "manifest.json"
        manifest_path.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        os.chmod(manifest_path, 0o644)

        archive = self.assets / qualification_tool.archive_name(
            self.version, target, archive_format
        )
        release_tool.create_archive(package, archive, archive_format)
        return archive

    def _write_target_sbom(self, target: str) -> Path:
        document = {
            "spdxVersion": "SPDX-2.3",
            "dataLicense": "CC0-1.0",
            "SPDXID": "SPDXRef-DOCUMENT",
            "name": f"Kaduox-SSH-{self.version}-{target}-Cargo-metadata",
            "documentNamespace": (
                "https://github.com/kaduoxzero/Kaduox-SSH/sbom/"
                f"{quote(self.tag, safe='-._~')}/{quote(target, safe='-._~')}/cargo-metadata"
            ),
            "creationInfo": {
                "created": "1970-01-01T00:00:00Z",
                "creators": ["Tool: test fixture"],
                "comment": f"Target-specific Cargo graph for {target} generated with --filter-platform.",
            },
            "packages": [
                {
                    "SPDXID": "SPDXRef-Kaduox-SSH-Release",
                    "name": "Kaduox-SSH",
                    "versionInfo": self.version,
                    "primaryPackagePurpose": "APPLICATION",
                    "comment": f"Cargo release target: {target}",
                },
                {
                    "SPDXID": "SPDXRef-Package-kaduox-ssh-cli-test",
                    "name": "kaduox-ssh-cli",
                    "versionInfo": self.version,
                },
                {
                    "SPDXID": "SPDXRef-Package-kaduox-ssh-core-test",
                    "name": "kaduox-ssh-core",
                    "versionInfo": self.version,
                },
            ],
            "relationships": [
                {
                    "spdxElementId": "SPDXRef-DOCUMENT",
                    "relationshipType": "DESCRIBES",
                    "relatedSpdxElement": "SPDXRef-Kaduox-SSH-Release",
                },
                {
                    "spdxElementId": "SPDXRef-Kaduox-SSH-Release",
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": "SPDXRef-Package-kaduox-ssh-cli-test",
                },
                {
                    "spdxElementId": "SPDXRef-Package-kaduox-ssh-cli-test",
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": "SPDXRef-Package-kaduox-ssh-core-test",
                },
            ],
        }
        path = self.assets / qualification_tool.sbom_name(self.version, target)
        path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return path

    def _populate_release(self) -> None:
        for target, (archive_format, exe_suffix) in qualification_tool.EXPECTED_TARGETS.items():
            self._write_package(target, archive_format, exe_suffix)
            self._write_target_sbom(target)
        self._write_sums()

    def _write_sums(self) -> None:
        names = sorted(qualification_tool.expected_release_assets(self.tag))
        text = "".join(
            f"{qualification_tool.sha256(self.assets / name)}  {name}\n" for name in names
        )
        (self.assets / "SHA256SUMS").write_text(text, encoding="utf-8")

    def test_complete_release_set_and_tar_zip_targets_qualify(self) -> None:
        self._populate_release()
        sums = qualification_tool.verify_release_asset_set(self.tag, self.assets)
        self.assertEqual(set(sums), qualification_tool.expected_release_assets(self.tag))
        self.assertEqual(len(sums), 8)

        linux_dir = qualification_tool.qualify_target(
            self.tag,
            "x86_64-unknown-linux-gnu",
            "tar.gz",
            "",
            self.assets,
            self.root / "qualified-linux",
        )
        self.assertTrue((linux_dir / "kssh").is_file())

        windows_dir = qualification_tool.qualify_target(
            self.tag,
            "x86_64-pc-windows-msvc",
            "zip",
            ".exe",
            self.assets,
            self.root / "qualified-windows",
        )
        self.assertTrue((windows_dir / "kssh.exe").is_file())

    def test_target_sbom_identity_tamper_is_rejected_even_with_fresh_checksum(self) -> None:
        self._populate_release()
        target = "x86_64-unknown-linux-gnu"
        sbom = self.assets / qualification_tool.sbom_name(self.version, target)
        document = json.loads(sbom.read_text(encoding="utf-8"))
        document["packages"][0]["comment"] = "Cargo release target: x86_64-pc-windows-msvc"
        sbom.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        self._write_sums()
        with self.assertRaisesRegex(ValueError, "root package"):
            qualification_tool.verify_release_asset_set(self.tag, self.assets)

    def test_legacy_release_wide_sbom_is_rejected_as_unexpected(self) -> None:
        self._populate_release()
        (self.assets / f"kaduox-ssh-{self.version}.spdx.json").write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "asset set differs"):
            qualification_tool.verify_release_asset_set(self.tag, self.assets)

    def test_checksum_tamper_is_rejected(self) -> None:
        self._populate_release()
        archive = self.assets / qualification_tool.archive_name(
            self.version, "x86_64-unknown-linux-gnu", "tar.gz"
        )
        with archive.open("ab") as handle:
            handle.write(b"tamper")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            qualification_tool.verify_release_asset_set(self.tag, self.assets)

    def test_unexpected_release_asset_is_rejected(self) -> None:
        self._populate_release()
        (self.assets / "unexpected.bin").write_bytes(b"unexpected")
        with self.assertRaisesRegex(ValueError, "asset set differs"):
            qualification_tool.verify_release_asset_set(self.tag, self.assets)

    def test_duplicate_checksum_entry_is_rejected(self) -> None:
        self._populate_release()
        sums = self.assets / "SHA256SUMS"
        first = sums.read_text(encoding="utf-8").splitlines()[0]
        sums.write_text(sums.read_text(encoding="utf-8") + first + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate asset"):
            qualification_tool.parse_sha256sums(sums)

    def test_tar_path_traversal_is_rejected_before_extraction(self) -> None:
        root = qualification_tool.package_name(
            self.version, "x86_64-unknown-linux-gnu"
        )
        archive = self.root / "traversal.tar.gz"
        with tarfile.open(archive, "w:gz") as handle:
            directory = tarfile.TarInfo(root)
            directory.type = tarfile.DIRTYPE
            directory.mode = 0o755
            handle.addfile(directory)
            payload = b"escape"
            member = tarfile.TarInfo(f"{root}/../escape")
            member.size = len(payload)
            member.mode = 0o644
            handle.addfile(member, io.BytesIO(payload))
        with self.assertRaisesRegex(ValueError, "escapes the package root"):
            qualification_tool.extract_release_archive(
                archive,
                self.root / "extract-traversal",
                self.version,
                "x86_64-unknown-linux-gnu",
                "tar.gz",
                "",
            )
        self.assertFalse((self.root / "escape").exists())

    def test_tar_symlink_member_is_rejected(self) -> None:
        root = qualification_tool.package_name(
            self.version, "x86_64-unknown-linux-gnu"
        )
        archive = self.root / "symlink.tar.gz"
        with tarfile.open(archive, "w:gz") as handle:
            directory = tarfile.TarInfo(root)
            directory.type = tarfile.DIRTYPE
            directory.mode = 0o755
            handle.addfile(directory)
            member = tarfile.TarInfo(f"{root}/kssh")
            member.type = tarfile.SYMTYPE
            member.linkname = "/bin/sh"
            member.mode = 0o755
            handle.addfile(member)
        with self.assertRaisesRegex(ValueError, "unsupported member type"):
            qualification_tool.extract_release_archive(
                archive,
                self.root / "extract-symlink",
                self.version,
                "x86_64-unknown-linux-gnu",
                "tar.gz",
                "",
            )

    def test_manifest_binary_checksum_mismatch_is_rejected(self) -> None:
        target = "x86_64-unknown-linux-gnu"
        archive_format = "tar.gz"
        exe_suffix = ""
        name = qualification_tool.package_name(self.version, target)
        package = self.root / "bad-manifest-stage" / name
        package.mkdir(parents=True)
        entries = []
        for binary in release_tool.EXPECTED_BINARIES:
            path = package / binary
            path.write_text(
                f"#!/bin/sh\nprintf '%s\\n' '{binary} {self.version}'\n",
                encoding="utf-8",
            )
            os.chmod(path, 0o755)
            entries.append(
                {
                    "name": binary,
                    "file": binary,
                    "sha256": "0" * 64 if binary == "kssh" else qualification_tool.sha256(path),
                    "bytes": path.stat().st_size,
                }
            )
        for document in ("README.md", "README.zh-CN.md", "LICENSE"):
            (package / document).write_text("doc\n", encoding="utf-8")
        (package / "manifest.json").write_text(
            json.dumps(
                {
                    "schema": 1,
                    "project": "Kaduox-SSH",
                    "tag": self.tag,
                    "version": self.version,
                    "target": target,
                    "binaries": entries,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        archive = self.root / "bad-manifest.tar.gz"
        release_tool.create_archive(package, archive, archive_format)
        extracted = qualification_tool.extract_release_archive(
            archive,
            self.root / "extract-bad-manifest",
            self.version,
            target,
            archive_format,
            exe_suffix,
        )
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            qualification_tool.validate_manifest(
                extracted, self.tag, self.version, target, exe_suffix
            )

    def test_nonempty_extract_directory_is_rejected(self) -> None:
        self._populate_release()
        extract = self.root / "not-empty"
        extract.mkdir()
        (extract / "sentinel").write_text("keep", encoding="utf-8")
        archive = self.assets / qualification_tool.archive_name(
            self.version, "x86_64-unknown-linux-gnu", "tar.gz"
        )
        with self.assertRaisesRegex(ValueError, "must be empty"):
            qualification_tool.extract_release_archive(
                archive,
                extract,
                self.version,
                "x86_64-unknown-linux-gnu",
                "tar.gz",
                "",
            )
        self.assertEqual((extract / "sentinel").read_text(encoding="utf-8"), "keep")

    def test_archive_modes_remain_part_of_contract(self) -> None:
        self._populate_release()
        archive = self.assets / qualification_tool.archive_name(
            self.version, "x86_64-unknown-linux-gnu", "tar.gz"
        )
        with tarfile.open(archive, "r:gz") as handle:
            modes = {
                member.name: stat.S_IMODE(member.mode) for member in handle.getmembers()
            }
        package = qualification_tool.package_name(
            self.version, "x86_64-unknown-linux-gnu"
        )
        self.assertEqual(modes[package], 0o755)
        self.assertEqual(modes[f"{package}/kssh"], 0o755)
        self.assertEqual(modes[f"{package}/manifest.json"], 0o644)


if __name__ == "__main__":
    unittest.main()
