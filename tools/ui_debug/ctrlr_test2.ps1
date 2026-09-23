# Ctrl+R test with AttachThreadInput (guaranteed foreground).
param([string]$LogPath = "$PSScriptRoot\..\..\target\ui_trace.log")
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W6 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
[W6]::SetProcessDPIAware() | Out-Null
if (Test-Path $LogPath) { Remove-Item $LogPath }
$env:ZERON_UI_TRACE = "1"
$exe = "$PSScriptRoot\..\..\target\debug\zeron.exe"
$proc = Start-Process -FilePath $exe -WorkingDirectory "$PSScriptRoot\..\.." -RedirectStandardError $LogPath -RedirectStandardOutput "$PSScriptRoot\..\..\target\ctrl.stdout.log" -PassThru
try {
  Start-Sleep -Seconds 25
  $hwnd = $proc.MainWindowHandle
  # Attach our input thread to the app's so foreground changes stick.
  $pid2 = 0
  $appThread = [W6]::GetWindowThreadProcessId($hwnd, [ref]$pid2)
  $myThread = [W6]::GetCurrentThreadId()
  [W6]::AttachThreadInput($myThread, $appThread, $true) | Out-Null
  [W6]::ShowWindow($hwnd, 5) | Out-Null   # SW_SHOW
  [W6]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 500
  # A real click at the window's center first: physical input reliably
  # grants focus even when the terminal holds the foreground lock.
  Add-Type -AssemblyName System.Windows.Forms
  $fg = [System.Windows.Forms.Cursor]::Position
  $r = $proc.MainWindowHandle
  [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point(700, 500)
  Start-Sleep -Milliseconds 100
  [W6]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
  [W6]::keybd_event(0x52, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [W6]::keybd_event(0x52, 0, 2, [UIntPtr]::Zero)
  [W6]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  [W6]::keybd_event(0x52, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [W6]::keybd_event(0x52, 0, 2, [UIntPtr]::Zero)
  [W6]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Seconds 4
  [W6]::AttachThreadInput($myThread, $appThread, $false) | Out-Null
  Write-Output "--- last traces:"
  Get-Content $LogPath -ErrorAction SilentlyContinue | Select-Object -Last 5
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
