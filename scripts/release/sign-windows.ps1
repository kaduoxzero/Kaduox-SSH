param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryDir
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$required = @(
    "KADUOX_WINDOWS_SIGNING_PFX_BASE64",
    "KADUOX_WINDOWS_SIGNING_PFX_PASSWORD",
    "KADUOX_WINDOWS_TIMESTAMP_URL"
)
foreach ($name in $required) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ([string]::IsNullOrWhiteSpace($value)) {
        throw "required native-signing input $name is missing"
    }
}

$binaryRoot = (Resolve-Path -LiteralPath $BinaryDir).Path
$expected = @("kssh.exe", "kssh-tui.exe", "kssh-fleet.exe", "kssh-inventory.exe")
foreach ($name in $expected) {
    $path = Join-Path $binaryRoot $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "release binary is missing before Authenticode signing: $path"
    }
}

$signTool = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -File |
    Sort-Object -Property FullName -Descending |
    Select-Object -First 1
if ($null -eq $signTool) {
    throw "signtool.exe was not found in the Windows SDK"
}

$pfxPath = Join-Path $env:RUNNER_TEMP "kaduox-signing.pfx"
try {
    $pfxBytes = [Convert]::FromBase64String($env:KADUOX_WINDOWS_SIGNING_PFX_BASE64)
    [IO.File]::WriteAllBytes($pfxPath, $pfxBytes)

    foreach ($name in $expected) {
        $path = Join-Path $binaryRoot $name
        & $signTool.FullName sign /fd SHA256 /tr $env:KADUOX_WINDOWS_TIMESTAMP_URL /td SHA256 /f $pfxPath /p $env:KADUOX_WINDOWS_SIGNING_PFX_PASSWORD $path
        if ($LASTEXITCODE -ne 0) {
            throw "signtool sign failed for $name with exit code $LASTEXITCODE"
        }
        & $signTool.FullName verify /pa /v /tw $path
        if ($LASTEXITCODE -ne 0) {
            throw "signtool verify failed for $name with exit code $LASTEXITCODE"
        }
    }
}
finally {
    if (Test-Path -LiteralPath $pfxPath) {
        Remove-Item -LiteralPath $pfxPath -Force
    }
}
