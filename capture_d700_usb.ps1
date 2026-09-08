# Capture the Asparion D700's USB hub while you drive the Configurator.
# The D700 sits on \.\USBPcap3 (root hub 3.0, port 3): a composite device with
# a vendor-defined HID interface (MI_00) and USB-MIDI (MI_01). The HID side is
# the Configurator's private channel and the reason for this capture.
#
# MUST be run elevated - USBPcap needs admin to open its control device.
#
# Usage:  powershell -ExecutionPolicy Bypass -File capture_d700_usb.ps1 [seconds] [hubNumber]

param([int]$Seconds = 30, [int]$Hub = 3)

$exe = "$env:ProgramFiles\USBPcap\USBPcapCMD.exe"
if (-not (Test-Path $exe)) { Write-Error "USBPcapCMD not found at $exe"; exit 1 }

$pr = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $pr.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Not elevated. Right-click the terminal and Run as Administrator."
    exit 1
}

$outDir = "D:\dev\S21_HiJack\usbcap"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$out = Join-Path $outDir "d700_hub$Hub.pcap"
if (Test-Path $out) { Remove-Item $out -Force }

$dev = '\\.\USBPcap' + $Hub
Write-Host "capturing $dev -> $out"
$p = Start-Process -FilePath $exe -ArgumentList @('-d', $dev, '-A', '-o', $out) `
     -PassThru -WindowStyle Hidden
Start-Sleep -Milliseconds 700
if ($p.HasExited) { Write-Error "USBPcapCMD exited immediately. Try another hub: -Hub 1/2/4/5"; exit 1 }

Write-Host ""
Write-Host "==================================================================="
Write-Host " NOW: in the Asparion Configurator, change the MASTER DIAL COLOUR."
Write-Host " One clean change (e.g. black -> red). Touch nothing else."
Write-Host " Capturing for $Seconds seconds..."
Write-Host "==================================================================="
for ($i = $Seconds; $i -gt 0; $i--) {
    Write-Host -NoNewline "`r  $i s remaining   "
    Start-Sleep -Seconds 1
}
Write-Host ""

if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
Start-Sleep -Milliseconds 600

if (Test-Path $out) {
    $len = (Get-Item $out).Length
    Write-Host ""
    Write-Host ("captured: {0}  ({1:N0} bytes)" -f $out, $len)
    if ($len -lt 200) { Write-Host "  WARNING: file is nearly empty - no traffic seen on this hub." }
    Write-Host "Done. Tell Claude the capture is ready."
} else {
    Write-Error "No capture file produced."
}
