param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryDir
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$binaryRoot = (Resolve-Path -LiteralPath $BinaryDir).Path
$expected = @("kssh.exe", "kssh-tui.exe", "kssh-fleet.exe", "kssh-inventory.exe")
$signTool = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -File |
    Sort-Object -Property FullName -Descending |
    Select-Object -First 1
if ($null -eq $signTool) {
    throw "signtool.exe was not found in the Windows SDK"
}

foreach ($name in $expected) {
    $path = Join-Path $binaryRoot $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "published Windows binary is missing: $path"
    }
    & $signTool.FullName verify /pa /v /tw $path
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode verification failed for $name with exit code $LASTEXITCODE"
    }
}
