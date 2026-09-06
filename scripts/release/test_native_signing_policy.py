from __future__ import annotations

import unittest

from scripts.release import release_tool


RELEASE = release_tool.ROOT / ".github" / "workflows" / "release.yml"
QUALIFICATION = release_tool.ROOT / ".github" / "workflows" / "release-qualification.yml"
WINDOWS_SIGN = release_tool.ROOT / "scripts" / "release" / "sign-windows.ps1"
WINDOWS_VERIFY = release_tool.ROOT / "scripts" / "release" / "verify-windows-signatures.ps1"
MACOS_SIGN = release_tool.ROOT / "scripts" / "release" / "sign-macos.sh"
MACOS_VERIFY = release_tool.ROOT / "scripts" / "release" / "verify-macos-signatures.sh"


class NativeSigningPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.release = RELEASE.read_text(encoding="utf-8")
        self.qualification = QUALIFICATION.read_text(encoding="utf-8")
        self.windows_sign = WINDOWS_SIGN.read_text(encoding="utf-8")
        self.windows_verify = WINDOWS_VERIFY.read_text(encoding="utf-8")
        self.macos_sign = MACOS_SIGN.read_text(encoding="utf-8")
        self.macos_verify = MACOS_VERIFY.read_text(encoding="utf-8")

    def test_stable_tags_require_native_signing_and_prereleases_are_opt_in(self) -> None:
        self.assertIn(
            "KADUOX_NATIVE_SIGNING_ENABLED: ${{ !contains(github.ref_name, '-') || vars.KADUOX_ENABLE_NATIVE_SIGNING == 'true' }}",
            self.release,
        )
        self.assertIn(
            "if: ${{ env.KADUOX_NATIVE_SIGNING_ENABLED == 'true' && runner.os == 'Windows' }}",
            self.release,
        )
        self.assertIn(
            "if: ${{ env.KADUOX_NATIVE_SIGNING_ENABLED == 'true' && runner.os == 'macOS' }}",
            self.release,
        )

    def test_signing_occurs_before_smoke_and_packaging(self) -> None:
        build = self.release.index("- name: Build all release binaries")
        windows = self.release.index("- name: Authenticode-sign Windows release binaries")
        macos = self.release.index("- name: Developer ID-sign and notarize macOS release binaries")
        smoke = self.release.index("- name: Smoke-test every release binary")
        package = self.release.index("- name: Package release suite")
        self.assertLess(build, windows)
        self.assertLess(build, macos)
        self.assertLess(windows, smoke)
        self.assertLess(macos, smoke)
        self.assertLess(smoke, package)

    def test_windows_credentials_are_secret_backed_and_timestamp_is_explicit(self) -> None:
        self.assertIn(
            "KADUOX_WINDOWS_SIGNING_PFX_BASE64: ${{ secrets.KADUOX_WINDOWS_SIGNING_PFX_BASE64 }}",
            self.release,
        )
        self.assertIn(
            "KADUOX_WINDOWS_SIGNING_PFX_PASSWORD: ${{ secrets.KADUOX_WINDOWS_SIGNING_PFX_PASSWORD }}",
            self.release,
        )
        self.assertIn(
            "KADUOX_WINDOWS_TIMESTAMP_URL: ${{ vars.KADUOX_WINDOWS_TIMESTAMP_URL }}",
            self.release,
        )
        self.assertIn("sign /fd SHA256 /tr", self.windows_sign)
        self.assertIn("/td SHA256 /f", self.windows_sign)
        self.assertIn("verify /pa /v /tw", self.windows_sign)
        self.assertIn("verify /pa /v /tw", self.windows_verify)
        self.assertNotIn("BEGIN CERTIFICATE", self.windows_sign)

    def test_macos_credentials_are_secret_backed_and_notary_uses_api_key(self) -> None:
        for required in (
            "KADUOX_MACOS_SIGNING_CERT_P12_BASE64: ${{ secrets.KADUOX_MACOS_SIGNING_CERT_P12_BASE64 }}",
            "KADUOX_MACOS_SIGNING_CERT_PASSWORD: ${{ secrets.KADUOX_MACOS_SIGNING_CERT_PASSWORD }}",
            "KADUOX_APPLE_NOTARY_KEY_P8_BASE64: ${{ secrets.KADUOX_APPLE_NOTARY_KEY_P8_BASE64 }}",
            "KADUOX_MACOS_SIGNING_IDENTITY: ${{ vars.KADUOX_MACOS_SIGNING_IDENTITY }}",
            "KADUOX_APPLE_NOTARY_KEY_ID: ${{ vars.KADUOX_APPLE_NOTARY_KEY_ID }}",
            "KADUOX_APPLE_NOTARY_ISSUER_ID: ${{ vars.KADUOX_APPLE_NOTARY_ISSUER_ID }}",
        ):
            self.assertIn(required, self.release)
        self.assertIn("codesign --force --timestamp --options runtime --sign", self.macos_sign)
        self.assertIn("xcrun notarytool submit", self.macos_sign)
        self.assertIn('--key "$notary_key"', self.macos_sign)
        self.assertIn('--key-id "$KADUOX_APPLE_NOTARY_KEY_ID"', self.macos_sign)
        self.assertIn('--issuer "$KADUOX_APPLE_NOTARY_ISSUER_ID"', self.macos_sign)
        self.assertIn("--wait", self.macos_sign)
        self.assertIn('status != "Accepted"', self.macos_sign)
        self.assertNotIn("--apple-id", self.macos_sign)
        self.assertNotIn("--password", self.macos_sign)

    def test_published_signed_assets_are_verified_without_private_credentials(self) -> None:
        expected_condition = (
            "(!contains(env.KADUOX_RELEASE_TAG, '-') || vars.KADUOX_ENABLE_NATIVE_SIGNING == 'true')"
        )
        self.assertIn(expected_condition, self.qualification)
        self.assertIn("verify-windows-signatures.ps1", self.qualification)
        self.assertIn("verify-macos-signatures.sh", self.qualification)
        self.assertIn("codesign --verify --strict --verbose=2", self.macos_verify)
        self.assertIn("spctl -vvv --assess --type exec", self.macos_verify)
        self.assertIn("verify /pa /v /tw", self.windows_verify)
        self.assertNotIn("secrets.KADUOX_", self.qualification)

    def test_private_key_material_is_always_temporary_and_cleaned(self) -> None:
        self.assertIn('Remove-Item -LiteralPath $pfxPath -Force', self.windows_sign)
        self.assertIn('security delete-keychain "$keychain"', self.macos_sign)
        self.assertIn('rm -rf "$work"', self.macos_sign)
        self.assertIn("trap cleanup EXIT", self.macos_sign)


if __name__ == "__main__":
    unittest.main()
