# Manual Ctrl+R test: boot, send Ctrl+R once, dump trace lines.
param([string]$LogPath = "$PSScriptRoot\..\..\target\ui_trace.log")
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W5 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@
[W5]::SetProcessDPIAware() | Out-Null
if (Test-Path $LogPath) { Remove-Item $LogPath }
$env:SKYLARK_UI_TRACE = "1"
$exe = "$PSScriptRoot\..\..\target\debug\skylark.exe"
$proc = Start-Process -FilePath $exe -WorkingDirectory "$PSScriptRoot\..\.." -RedirectStandardError $LogPath -RedirectStandardOutput "$PSScriptRoot\..\..\target\ctrl.stdout.log" -PassThru
try {
  Start-Sleep -Seconds 22
  $hwnd = $proc.MainWindowHandle
  [W5]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 500
  [W5]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
  [W5]::keybd_event(0x52, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [W5]::keybd_event(0x52, 0, 2, [UIntPtr]::Zero)
  [W5]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 1500
  # second attempt
  [W5]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
  [W5]::keybd_event(0x52, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [W5]::keybd_event(0x52, 0, 2, [UIntPtr]::Zero)
  [W5]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Seconds 3
  Write-Output "--- trace lines after Ctrl+R:"
  Get-Content $LogPath | Select-Object -Last 6
  Write-Output "--- boot-select tail:"
  (Select-String -Path $LogPath -Pattern "boot-select" | Select-Object -Last 2).Line
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
