param()
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W7 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder sb, int max);
}
"@
[W7]::SetProcessDPIAware() | Out-Null
$env:ZERON_UI_TRACE = "1"
$exe = "C:\Users\BiuBiu\Documents\my_app\zeron\target\debug\zeron.exe"
$log = "C:\Users\BiuBiu\Documents\my_app\zeron\target\ui_trace.log"
$proc = Start-Process -FilePath $exe -WorkingDirectory "C:\Users\BiuBiu\Documents\my_app\zeron" -RedirectStandardError $log -RedirectStandardOutput "C:\Users\BiuBiu\Documents\my_app\zeron\target\fg.stdout.log" -PassThru
Start-Sleep -Seconds 25
$hwnd = $proc.MainWindowHandle
function Title([IntPtr]$h) {
  $sb = New-Object System.Text.StringBuilder 256
  [W7]::GetWindowText($h, $sb, 256) | Out-Null
  $sb.ToString()
}
Write-Output ("before: fg = '{0}' target = '{1}'" -f (Title ([W7]::GetForegroundWindow())), (Title $hwnd))
$pid2 = 0
$appThread = [W7]::GetWindowThreadProcessId($hwnd, [ref]$pid2)
$myThread = [W7]::GetCurrentThreadId()
[W7]::AttachThreadInput($myThread, $appThread, $true) | Out-Null
[W7]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 300
Write-Output ("after : fg = '{0}'" -f (Title ([W7]::GetForegroundWindow())))
[W7]::AttachThreadInput($myThread, $appThread, $false) | Out-Null
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
