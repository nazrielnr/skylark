# PostMessage WM_KEYDOWN + physical modifier state: the keystroke reaches
# the app regardless of which window holds the foreground.
param([string]$LogPath = "$PSScriptRoot\..\..\target\ui_trace.log")
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W9 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll", CharSet=CharSet.Auto)] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@
[W9]::SetProcessDPIAware() | Out-Null
if (Test-Path $LogPath) { Remove-Item $LogPath }
$env:SKYLARK_UI_TRACE = "1"
$exe = "$PSScriptRoot\..\..\target\debug\skylark.exe"
$proc = Start-Process -FilePath $exe -WorkingDirectory "$PSScriptRoot\..\.." -RedirectStandardError $LogPath -RedirectStandardOutput "$PSScriptRoot\..\..\target\pm.stdout.log" -PassThru
try {
  Start-Sleep -Seconds 25
  $hwnd = $proc.MainWindowHandle
  # Physical ctrl-down updates the global key state; the posted key goes to
  # the app's message queue directly.
  [W9]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)     # ctrl down
  Start-Sleep -Milliseconds 80
  $ok = [W9]::PostMessage($hwnd, 0x0100, [IntPtr]0x52, [IntPtr]0)
  Write-Output ("post keydown ok={0} hwnd={1}" -f $ok, $hwnd)
  Start-Sleep -Milliseconds 60
  [W9]::PostMessage($hwnd, 0x0101, [IntPtr]0x52, [IntPtr]0) | Out-Null   # WM_KEYUP 'R'
  [W9]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)     # ctrl up
  Start-Sleep -Seconds 4
  Write-Output "--- last traces:"
  Get-Content $LogPath -ErrorAction SilentlyContinue | Select-Object -Last 4
} finally {
  [W9]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
