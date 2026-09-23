# Read-only Windows snapshot. Never interpolates user input.
# Emits the same __KADUOX_SYSTEM_METRICS_V1__ key=value wire format as metrics.sh.
$ErrorActionPreference = 'SilentlyContinue'

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add('__KADUOX_SYSTEM_METRICS_V1__')

$os  = Get-CimInstance Win32_OperatingSystem
$cs  = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1

$lines.Add("hostname=$env:COMPUTERNAME")
$caption = ($os.Caption -replace 'Microsoft ', '').Trim()
$lines.Add("platform=$caption $($os.Version) $env:PROCESSOR_ARCHITECTURE")
$lines.Add("username=$env:USERNAME")

$up = (Get-Date) - $os.LastBootUpTime
$uptime = if ($up.Days -gt 0) { "up $($up.Days) days, $($up.Hours) hours" }
          elseif ($up.Hours -gt 0) { "up $($up.Hours) hours, $($up.Minutes) minutes" }
          else { "up $($up.Minutes) minutes" }
$lines.Add("uptime=$uptime")

$addrs = @(
  Get-NetIPAddress -AddressFamily IPv4 |
    Where-Object { $_.IPAddress -ne '127.0.0.1' -and $_.PrefixOrigin -ne 'WellKnown' } |
    Select-Object -ExpandProperty IPAddress
) -join ' '
$lines.Add("addresses=$addrs")

if ($cpu) { $lines.Add("cpu_model=$($cpu.Name.Trim())") }
if ($cs.NumberOfLogicalProcessors) { $lines.Add("cpu_cores=$($cs.NumberOfLogicalProcessors)") }

# Pre-formatted percent snapshot; sample twice to avoid first-read spikes.
$null = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'"
Start-Sleep -Milliseconds 300
$usage = (Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime
if ($null -ne $usage) { $lines.Add("cpu_usage=$usage") }

$totalMem = [uint64]$os.TotalVisibleMemorySize * 1KB
$freeMem  = [uint64]$os.FreePhysicalMemory * 1KB
$usedMem  = if ($totalMem -gt $freeMem) { $totalMem - $freeMem } else { [uint64]0 }
$lines.Add("memory=$totalMem $usedMem $freeMem")

Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object {
  $size = [uint64]$_.Size
  if ($size -gt 0) {
    $free = [uint64]$_.FreeSpace
    $used = $size - $free
    $pct  = [math]::Round(100.0 * $used / $size, 1)
    $lines.Add("disk=$($_.DeviceID) $size $used $free $pct")
  }
}

# GPU: reuse nvidia-smi when the driver is present; otherwise omit GPU lines.
$smi = Get-Command nvidia-smi.exe | Select-Object -First 1
if ($smi) {
  & $smi.Source --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits |
    ForEach-Object { $lines.Add("gpu=$_") }
}

$rx = [uint64]0
$tx = [uint64]0
Get-NetAdapterStatistics | ForEach-Object {
  $rx += [uint64]$_.ReceivedBytes
  $tx += [uint64]$_.SentBytes
}
$lines.Add("network=$rx $tx")

# Write raw UTF-8 bytes to stdout: console OutputEncoding is not
# controllable over an SSH pipe. Keep this script pure ASCII so it can be
# fed verbatim through stdin (`powershell -Command -`).
$stdout = [System.IO.StreamWriter]::new([Console]::OpenStandardOutput(), [System.Text.UTF8Encoding]::new($false))
$stdout.AutoFlush = $true
$stdout.Write(($lines -join "`n") + "`n")
$stdout.Dispose()
