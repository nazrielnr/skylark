# Visual bleed check v2: compares a mid-cover screenshot against a
# settled screenshot. During the cover the tree overlay must hide the file
# content completely: the covered region's pixels should be dominated by
# the opaque tree background, not by the file text that was there before.
# Heuristic: bright-pixel ratio in the OLD PREVIEW region must DROP
# sharply once the tree covers it (tree rows are sparse text on dark bg;
# file text is dense bright text).
param(
  [string]$Exe = "$PSScriptRoot\..\..\target\debug\zeron.exe",
  [string]$LogPath = "$PSScriptRoot\..\..\target\ui_trace.log",
  [string]$ShotDir = "$PSScriptRoot\..\..\target"
)
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct P3 { public int X, Y; }
public class W4 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref P3 pt);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern uint GetDpiForSystem();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
}
"@
[W4]::SetProcessDPIAware() | Out-Null
if (Test-Path $LogPath) { Remove-Item $LogPath }
$env:ZERON_UI_TRACE = "1"
$proc = Start-Process -FilePath $Exe -WorkingDirectory "$PSScriptRoot\..\.." -RedirectStandardError $LogPath -RedirectStandardOutput "$ShotDir\bleed.stdout.log" -PassThru
try {
  Start-Sleep -Seconds 20
  $hwnd = $proc.MainWindowHandle
  [W4]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 400
  $scale = [W4]::GetDpiForSystem() / 96.0
  $origin = New-Object P3; $origin.X = 0; $origin.Y = 0
  [W4]::ClientToScreen($hwnd, [ref]$origin) | Out-Null

  function Last-Trace([string]$Pattern) {
    $l = Select-String -Path $LogPath -Pattern $Pattern
    if ($l) { $l.Line | Select-Object -Last 1 } else { $null }
  }
  function Click([int]$x, [int]$y) {
    [W4]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 40
    [W4]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 30
    [W4]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
  }
  function Nums([string]$Line, [string]$Re) {
    $m = [regex]::Matches($Line, $Re); $m[0].Groups
  }
  function Shot([string]$Path) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
  }
  function BrightRatio([string]$Path, [int]$x, [int]$y, [int]$w, [int]$h) {
    $bmp = New-Object System.Drawing.Bitmap($Path)
    $bright = 0; $total = 0
    for ($px = $x; $px -lt ($x + $w); $px += 3) {
      for ($py = $y; $py -lt ($y + $h); $py += 3) {
        $c = $bmp.GetPixel($px, $py)
        $lum = 0.299*$c.R + 0.587*$c.G + 0.114*$c.B
        $total++
        if ($lum -gt 120) { $bright++ }
      }
    }
    $bmp.Dispose()
    if ($total -eq 0) { return 0.0 }
    return 100.0 * $bright / $total
  }

  # open pane + Files
  [W4]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
  [W4]::keybd_event(0x52, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 60
  [W4]::keybd_event(0x52, 0, 2, [UIntPtr]::Zero)
  [W4]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 3000
  if (-not (Last-Trace "tree-rows")) {
    $card = Last-Trace "bounds files-card"
    $v = Nums $card 'L=(-?[\d.]+) T=(-?[\d.]+) R=(-?[\d.]+) B=(-?[\d.]+)'
    Click ([int]($origin.X + ([double]$v[1].Value + [double]$v[3].Value)/2 * $scale)) ([int]($origin.Y + ([double]$v[2].Value + [double]$v[4].Value)/2 * $scale))
    Start-Sleep -Seconds 4
  }

  # open a text-dense file (README.md, row ~21) for a bright preview
  $tr = Last-Trace "tree-rows"
  $g = Nums $tr 'count=(\d+) L=(-?[\d.]+) T=(-?[\d.]+) R=(-?[\d.]+) B=(-?[\d.]+)'
  $count = [int]$g[1].Value; $L = [double]$g[2].Value; $T = [double]$g[3].Value; $R = [double]$g[4].Value; $B = [double]$g[5].Value
  $paths = @()
  if ($tr -match "paths=(.*)$") {
    foreach ($e in ($Matches[1] -split "\|")) { $k, $p = $e -split ":", 2; $paths += $p }
  }
  $readmeIdx = [array]::IndexOf($paths, "README.md")
  if ($readmeIdx -lt 0) { $readmeIdx = 8 }
  $rowH = ($B - $T) / $count
  $treeX = [int]($origin.X + ($L + $R) / 2 * $scale)
  $rowY = [int]($origin.Y + ($T + ($readmeIdx + 0.5) * $rowH) * $scale)
  Click $treeX $rowY
  Start-Sleep -Seconds 2

  # The file preview region from the preview-body probe (exact bounds).
  $pb = Last-Trace "bounds preview-body"
  $pg = Nums $pb 'L=(-?[\d.]+) T=(-?[\d.]+) R=(-?[\d.]+) B=(-?[\d.]+)'
  $previewX = [int]($origin.X + ([double]$pg[1].Value + 10) * $scale)
  $previewY = [int]($origin.Y + ([double]$pg[2].Value + 40) * $scale)
  $previewW = [int](([double]$pg[3].Value - [double]$pg[1].Value - 20) * $scale)
  $previewH = [int](([double]$pg[4].Value - [double]$pg[2].Value - 80) * $scale)
  if ($previewW -lt 30) { $previewW = 30 }
  if ($previewH -lt 30) { $previewH = 30 }
  Shot "$ShotDir\bleed_before.png"
  $before = BrightRatio "$ShotDir\bleed_before.png" $previewX $previewY $previewW $previewH

  # click the Files tab; capture mid cover
  $tab = Last-Trace "bounds files-tab"
  $tg = Nums $tab 'L=(-?[\d.]+) T=(-?[\d.]+) R=(-?[\d.]+) B=(-?[\d.]+)'
  $tabX = [int]($origin.X + ([double]$tg[1].Value + [double]$tg[3].Value)/2 * $scale)
  $tabY = [int]($origin.Y + ([double]$tg[2].Value + [double]$tg[4].Value)/2 * $scale)
  Click $tabX $tabY
  Start-Sleep -Milliseconds 185
  Shot "$ShotDir\bleed_mid.png"
  $mid = BrightRatio "$ShotDir\bleed_mid.png" $previewX $previewY $previewW $previewH

  Write-Output ("preview-region bright ratio: before={0:N2}% mid-cover={1:N2}%" -f $before, $mid)
  if ($before -lt 2.0) {
    Write-Output "SKIP: preview had no visible text; inconclusive"
    exit 0
  }
  # With an opaque cover, the dense file text disappears: the ratio must
  # drop hard (tree rows are sparse). Bleed keeps it high.
  if ($mid -gt ($before * 0.6)) {
    Write-Output "FAIL: file text still visible through the covering tree"
    exit 1
  }
  Write-Output "PASS: tree covers the file content opaquely"
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
