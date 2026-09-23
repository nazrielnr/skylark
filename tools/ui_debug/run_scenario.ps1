# UI debug driver: launches the app with tracing enabled and drives it with
# real keyboard/mouse input, then asserts against the [trace] log lines.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tools/ui_debug/run_scenario.ps1
#   powershell ... -Scenario hover          # hover sweep only
#   powershell ... -Scenario tabs           # rapid tab open/switch
#
# Requires a debug build: cargo build -p zeron
# See docs/ui-tracing.md for the trace vocabulary.

param(
  [string]$Exe = "$PSScriptRoot\..\..\target\debug\zeron.exe",
  [string]$LogPath = "$PSScriptRoot\..\..\target\ui_trace.log",
  [string]$Scenario = "smoke",
  [int]$BootSeconds = 18
)

$ErrorActionPreference = "Stop"

Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct POINT { public int X, Y; }
public class Win {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT pt);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern uint GetDpiForSystem();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
}
"@
[Win]::SetProcessDPIAware() | Out-Null

if (-not (Test-Path $Exe)) { Write-Output "EXE MISSING: $Exe (cargo build -p zeron)"; exit 1 }
if (Test-Path $LogPath) { Remove-Item $LogPath }

$env:ZERON_UI_TRACE = "1"
$proc = Start-Process -FilePath $Exe -WorkingDirectory "$PSScriptRoot\..\.." `
  -RedirectStandardError $LogPath `
  -RedirectStandardOutput "$PSScriptRoot\..\..\target\ui_trace.stdout.log" -PassThru

try {
  Start-Sleep -Seconds $BootSeconds
  $hwnd = $proc.MainWindowHandle
  if ($hwnd -eq [IntPtr]::Zero) { Write-Output "NO WINDOW"; exit 1 }
  # -32000 = minimized/hidden: wait for the window to come up.
  $origin = New-Object POINT
  for ($try = 0; $try -lt 10; $try++) {
    $origin.X = 0; $origin.Y = 0
    [Win]::ClientToScreen($hwnd, [ref]$origin) | Out-Null
    if ($origin.X -ne -32000 -and $origin.Y -ne -32000) { break }
    Start-Sleep -Seconds 2
  }
  [Win]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds 400

  $scale = [Win]::GetDpiForSystem() / 96.0
  $origin = New-Object POINT; $origin.X = 0; $origin.Y = 0
  [Win]::ClientToScreen($hwnd, [ref]$origin) | Out-Null
  Write-Output ("client origin: {0},{1} scale: {2}" -f $origin.X, $origin.Y, $scale)

  function Send-CtrlR {
    [Win]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
    [Win]::keybd_event(0x52, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [Win]::keybd_event(0x52, 0, 2, [UIntPtr]::Zero)
    [Win]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
  }

  function Click-At([int]$x, [int]$y) {
    [Win]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 50
    [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
    [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
  }

  function Get-LastTrace([string]$Pattern) {
    $lines = Select-String -Path $LogPath -Pattern $Pattern
    if ($lines) { $lines.Line | Select-Object -Last 1 } else { $null }
  }

  function Parse-Numbers([string]$Line) {
    $map = @{}
    foreach ($m in [regex]::Matches($Line, '(\w+)=(-?\d[\d.]*)')) {
      $map[$m.Groups[1].Value] = [double]$m.Groups[2].Value
    }
    $map
  }

  # Open the right pane and, when the picker shows, open Files. Boot timing
  # varies with the engine connect, so retry Ctrl+R until something appears.
  for ($try = 0; $try -lt 4; $try++) {
    if ((Get-LastTrace "tree-rows") -or (Get-LastTrace "bounds files-card")) { break }
    Send-CtrlR
    Start-Sleep -Milliseconds 3000
  }
  if (-not (Get-LastTrace "tree-rows")) {
    $cardLine = Get-LastTrace "bounds files-card"
    if (-not $cardLine) { Write-Output "FAIL no tree and no files card after Ctrl+R"; exit 1 }
    $card = Parse-Numbers $cardLine
    $cx = [int]($origin.X + (($card.L + $card.R) / 2.0) * $scale)
    $cy = [int]($origin.Y + (($card.T + $card.B) / 2.0) * $scale)
    Write-Output "clicking files card at $cx,$cy"
    Click-At $cx $cy
    Start-Sleep -Seconds 4
  }

  $treeLine = Get-LastTrace "tree-rows"
  if (-not $treeLine) { Write-Output "FAIL no tree after opening Files"; exit 1 }
  Write-Output "tree: $treeLine"
  $tree = Parse-Numbers $treeLine
  $treeCenterX = [int]($origin.X + (($tree.L + $tree.R) / 2.0) * $scale)
  $treeTop = $origin.Y + $tree.T * $scale
  $rowCount = [int]$tree.count
  # Rows are uniform: total content height / count (logical -> physical).
  $rowHeight = if ($rowCount -gt 0 -and $tree.content_h -gt 0) {
    ($tree.content_h / $rowCount) * $scale
  } else { 46.5 }

  function Row-Y([int]$i) { [int]($treeTop + ($i + 0.5) * $rowHeight) }

  $failures = @()

  if ($Scenario -eq "smoke" -or $Scenario -eq "hover") {
    # --- Hover sweep -------------------------------------------------------
    $treeBottom = $origin.Y + $tree.B * $scale
    $before = (Get-Content $LogPath | Measure-Object -Line).Lines
    $pass = 0
    while ($pass -lt 3) {
      $y = $treeTop + 8
      while ($y -lt $treeBottom - 8) {
        [Win]::SetCursorPos($treeCenterX, [int]$y) | Out-Null
        $y += 6
        Start-Sleep -Milliseconds 4
      }
      $pass++
    }
    Start-Sleep -Milliseconds 800
    $after = (Get-Content $LogPath | Measure-Object -Line).Lines
    Write-Output ("hover sweep: +{0} trace lines" -f ($after - $before))
    if (($after - $before) -lt 3) { $failures += "hover produced no repaint activity" }
  }

  if ($Scenario -eq "search") {
    # --- Search flow ------------------------------------------------------
    # Open the search field, type a query, verify results render, the tree
    # does not reload, and the sidebar (split) mode shows the input too.
    $loadsBefore = (Select-String -Path $LogPath -Pattern "tree-load").Count
    $inL = Get-LastTrace "bounds search-input"
    if (-not $inL) {
      # Open the search field via the toolbar's magnifier toggle.
      $tog = Get-LastTrace "bounds search-toggle"
      if (-not $tog) {
        Write-Output "FAIL no search-toggle probe"
        $failures += "no search-toggle probe"
      } else {
        $st = Parse-Numbers $tog
        $stX = [int]($origin.X + (($st.L + $st.R) / 2.0) * $scale)
        $stY = [int]($origin.Y + (($st.T + $st.B) / 2.0) * $scale)
        Click-At $stX $stY
        Start-Sleep -Milliseconds 400
        $inL = Get-LastTrace "bounds search-input"
      }
    }
    if (-not $inL) {
      Write-Output "FAIL search input not visible"
      $failures += "search input not visible in raw mode"
    } else {
      $in = Parse-Numbers $inL
      $inX = [int]($origin.X + (($in.L + $in.R) / 2.0) * $scale)
      $inY = [int]($origin.Y + (($in.T + $in.B) / 2.0) * $scale)
      Click-At $inX $inY
      Start-Sleep -Milliseconds 600
      # type "readme" - README.md ranks first
      foreach ($ch in @(0x52, 0x45, 0x41, 0x44, 0x4D, 0x45)) {
        [Win]::keybd_event([byte]$ch, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 30
        [Win]::keybd_event([byte]$ch, 0, 2, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 60
      }
      Start-Sleep -Milliseconds 900
      $loadsAfter = (Select-String -Path $LogPath -Pattern "tree-load").Count
      Write-Output ("search typing: tree-loads delta {0}" -f ($loadsAfter - $loadsBefore))
      if (($loadsAfter - $loadsBefore) -ne 0) {
        $failures += "searching reloaded the workspace tree"
      }
      # Click the README.md result — uses the SEARCH rows' own trace (the
      # tree-rows line is stale while results replace the tree).
      $resLine = Get-LastTrace "search-rows"
      $resBox = Get-LastTrace "bounds search-results"
      if ($resLine -and $resLine -match "paths=(.*)$" -and $resBox) {
        $rp = $Matches[1] -split "\|"
        $readme = -1
        for ($i = 0; $i -lt $rp.Count; $i++) {
          $k, $pp = $rp[$i] -split ":", 2
          if ($pp -eq "README.md") { $readme = $i; break }
        }
        if ($readme -lt 0) {
          for ($i = 0; $i -lt $rp.Count; $i++) {
            $k, $pp = $rp[$i] -split ":", 2
            if ($k -eq "f" -and $pp -match "README") { $readme = $i; break }
          }
        }
        if ($readme -lt 0) {
          for ($i = 0; $i -lt $rp.Count; $i++) {
            $k, $pp = $rp[$i] -split ":", 2
            if ($k -eq "f") { $readme = $i; break }
          }
        }
        if ($readme -lt 0) { $readme = 0 }
        $rb = Parse-Numbers $resBox
        $rowH2 = 0.0
        if ($resLine -match "row_h=([0-9.]+)") { $rowH2 = [double]$Matches[1] }
        if ($rowH2 -le 0 -and $rb.count -gt 0) { $rowH2 = ($rb.B - $rb.T) / $rb.count }
        $rx = [int]($origin.X + (($rb.L + $rb.R) / 2.0) * $scale)
        $ry = [int]($origin.Y + ($rb.T + ($readme + 0.5) * $rowH2) * $scale)
        [Win]::SetCursorPos($rx, $ry) | Out-Null
        Start-Sleep -Milliseconds 40
        [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 30
        [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 900
        # Search-result clicks open via open_tree_file (no tree-open
        # trace — that fires on tree row clicks); the created tab is the
        # proof.
        $newTab = Get-LastTrace "surface-new kind=file"
        Write-Output "opened tab: $newTab"
        if (-not ($newTab -match "README")) {
          $failures += "search result click did not open README.md"
        } else {
          # In the split sidebar the search input must be visible...
          $inL2 = Get-LastTrace "bounds search-input"
          Write-Output ("split search-input: {0}" -f $inL2)
          if (-not $inL2) {
            $failures += "search input missing in the split sidebar"
          }
          # ...and the RAW surface's search state must survive the switch:
          # back to the Files tab, then to the file tab again.
          $tabL2 = Get-LastTrace "bounds files-tab"
          $tb2 = Parse-Numbers $tabL2
          $tb2X = [int]($origin.X + (($tb2.L + $tb2.R) / 2.0) * $scale)
          $tb2Y = [int]($origin.Y + (($tb2.T + $tb2.B) / 2.0) * $scale)
          Click-At $tb2X $tb2Y
          Start-Sleep -Milliseconds 900
          $rawStill = Get-LastTrace "search-rows"
          Write-Output ("raw search persists: {0}" -f [bool]$rawStill)
          if (-not $rawStill) {
            $failures += "raw search state reset after visiting it"
          }

          # The split sidebar must ALSO offer search: toggle it from the
          # sidebar toolbar and the input must render INSIDE the sidebar
          # (left edge past the preview area).
          $ftL = Get-LastTrace "surface-new kind=file"
          if ($ftL -match 'id=([0-9]+)') { $fileId = [int]$Matches[1] }
          # click the search toggle (it lives in the split header)
          $stog = Get-LastTrace "bounds search-toggle"
          $stg = Parse-Numbers $stog
          $stX = [int]($origin.X + (($stg.L + $stg.R) / 2.0) * $scale)
          $stY = [int]($origin.Y + (($stg.T + $stg.B) / 2.0) * $scale)
          # return to the file tab first
          $treeLineF = Get-LastTrace "tree-rows"
          $tf = Parse-Numbers $treeLineF
          $tfX = [int]($origin.X + (($tf.L + $tf.R) / 2.0) * $scale)
          # click the file TAB (not the tree): reuse the tab strip row
          $ftab = Get-LastTrace "bounds files-tab"
          $ftab2 = Get-LastTrace "bounds file-tab"
          $ftb = Parse-Numbers $ftab2
          $fileTabX = [int]($origin.X + (($ftb.L + $ftb.R) / 2.0) * $scale)
          $fileTabY = [int]($origin.Y + (($ftb.T + $ftb.B) / 2.0) * $scale)
          Click-At $fileTabX $fileTabY
          Start-Sleep -Milliseconds 700
          # Re-read the toggle from the SPLIT header's latest render (the
          # raw-mode line is stale geometry).
          $stog2 = Get-LastTrace "bounds search-toggle"
          $stg2 = Parse-Numbers $stog2
          $stX2 = [int]($origin.X + (($stg2.L + $stg2.R) / 2.0) * $scale)
          $stY2 = [int]($origin.Y + (($stg2.T + $stg2.B) / 2.0) * $scale)
          Click-At $stX2 $stY2
          Start-Sleep -Milliseconds 500
          $splitInput = Get-LastTrace "bounds search-input"
          Write-Output ("split search input: {0}" -f $splitInput)
          if ($splitInput) {
            $si = Parse-Numbers $splitInput
            if ($si.L -lt 1000) {
              $failures += ("split search input at L={0}, not inside the sidebar" -f $si.L)
            }
          } else {
            $failures += "search input missing in the split sidebar"
          }
        }
      } else {
        $failures += "no search results rendered"
      }
    }
  }

  if ($Scenario -eq "collapse") {
    # --- Collapse-aware transitions -------------------------------------
    # Open one file, collapse the tree, go raw (cover from=0), return to
    # the file tab (reveal to=0 — swipe to the corner, NOT expand).
    $selectsBefore = (Select-String -Path $LogPath -Pattern "tree-select").Count
    $treeLineC = Get-LastTrace "tree-rows"
    $treeC = Parse-Numbers $treeLineC
    $rowH = if ($treeC.count -gt 0 -and $treeC.content_h -gt 0) {
      ($treeC.content_h / $treeC.count) * $scale
    } else { 46.5 }
    # .env.example is row 9 from the paths list.
    $rowPaths = @(); $rowKinds = @()
    if ($treeLineC -match "paths=(.*)$") {
      foreach ($entry in ($Matches[1] -split "\|")) {
        $k, $pp = $entry -split ":", 2
        $rowKinds += $k; $rowPaths += $pp
      }
    }
    $fIdx = [array]::IndexOf($rowPaths, ".env.example")
    if ($fIdx -lt 0) { $fIdx = 9 }
    $fy = [int]($treeTop + ($fIdx + 0.5) * $rowH)
    [Win]::SetCursorPos($treeCenterX, $fy) | Out-Null
    Start-Sleep -Milliseconds 30
    [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 20
    [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 800

    # Collapse via the tree toggle.
    $tog = Get-LastTrace "bounds tree-toggle"
    if (-not $tog) { Write-Output "FAIL no tree-toggle probe"; exit 1 }
    $tg2 = Parse-Numbers $tog
    $togX = [int]($origin.X + (($tg2.L + $tg2.R) / 2.0) * $scale)
    $togY = [int]($origin.Y + (($tg2.T + $tg2.B) / 2.0) * $scale)
    Click-At $togX $togY
    Start-Sleep -Milliseconds 700

    # Raw: the Files tab (cover must start from the corner).
    $tabL = Get-LastTrace "bounds files-tab"
    $tb = Parse-Numbers $tabL
    $tbX = [int]($origin.X + (($tb.L + $tb.R) / 2.0) * $scale)
    $tbY = [int]($origin.Y + (($tb.T + $tb.B) / 2.0) * $scale)
    $coverBefore = (Select-String -Path $LogPath -Pattern "cover-expand").Count
    Click-At $tbX $tbY
    Start-Sleep -Milliseconds 700
    $coverLine = Get-LastTrace "cover-expand"

    # Back to the file tab: reveal must target 0 (swipe to the corner).
    $treeLineR = Get-LastTrace "tree-rows"
    $treeR2 = Parse-Numbers $treeLineR
    $fY2 = [int]($origin.Y + ($treeR2.T + ($fIdx + 0.5) * ($treeR2.content_h / $treeR2.count)) * $scale)
    $fX2 = [int]($origin.X + (($treeR2.L + $treeR2.R) / 2.0) * $scale)
    [Win]::SetCursorPos($fX2, $fY2) | Out-Null
    Start-Sleep -Milliseconds 30
    [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 20
    [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 700
    $revealLine = Get-LastTrace "sidebar-reveal"

    Write-Output "cover: $coverLine"
    Write-Output "reveal: $revealLine"
    if ($coverLine -and $coverLine -match "from=([0-9.]+)") {
      if ([double]$Matches[1] -gt 5.0) {
        $failures += ("cover started at {0}, not the collapsed corner" -f $Matches[1])
      }
    } else {
      $failures += "no cover on the raw switch"
    }
    if ($revealLine -and $revealLine -match "to=([0-9.]+)") {
      if ([double]$Matches[1] -gt 5.0) {
        $failures += ("reveal targeted {0}, not the collapsed corner" -f $Matches[1])
      }
    } else {
      $failures += "no reveal on the file switch"
    }
  }

  if ($Scenario -eq "smoke" -or $Scenario -eq "tabs") {
    # --- Rapid FILE-row clicks (root level, no expansion -> stable indices)
    $selectsBefore = (Select-String -Path $LogPath -Pattern "tree-select").Count
    $opensBefore = (Select-String -Path $LogPath -Pattern "tree-open").Count
    $fileTabsBefore = (Select-String -Path $LogPath -Pattern "surface-new kind=file").Count
    $rootLoadsBefore = (Select-String -Path $LogPath -Pattern 'tree-load dir=""').Count

    $rowPaths = @()
    $rowKinds = @()
    if ($treeLine -match "paths=(.*)$") {
      foreach ($entry in ($Matches[1] -split "\|")) {
        $kind, $path = $entry -split ":", 2
        $rowKinds += $kind
        $rowPaths += $path
      }
    }
    # Root-level FILE rows (kind 'f' from the trace).
    $fileRows = @()
    for ($i = 0; $i -lt $rowPaths.Count; $i++) {
      if ($rowKinds[$i] -eq "f") { $fileRows += $i }
    }
    if ($fileRows.Count -eq 0) { $fileRows = @($rowPaths.Count - 1) }
    $targets = $fileRows | Select-Object -First 3
    Write-Output ("file rows at root: {0}" -f (($targets | ForEach-Object { $rowPaths[$_] }) -join ", "))

    # Clicking a file opens an editor tab: the tree moves from the browser's
    # full-pane layout to the editor's narrow right sidebar (layout change).
    # Re-read the geometry after every click and re-find the row by path.
    foreach ($idx in $targets) {
      $targetPath = $rowPaths[$idx]
      # Wait for a post-click render, then use the newest geometry.
      Start-Sleep -Milliseconds 500
      $liveLine = Get-LastTrace "tree-rows"
      if ($liveLine) {
        $live = Parse-Numbers $liveLine
        $livePaths = @()
        if ($liveLine -match "paths=(.*)$") {
          foreach ($entry in ($Matches[1] -split "\|")) {
            $k, $p = $entry -split ":", 2
            $livePaths += $p
          }
        }
        $liveIdx = [array]::IndexOf($livePaths, $targetPath)
        if ($liveIdx -ge 0) {
          $liveH = if ($live.content_h -gt 0) { ($live.content_h / $live.count) * $scale } else { $rowHeight }
          $liveX = [int]($origin.X + (($live.L + $live.R) / 2.0) * $scale)
          $liveY = [int]($origin.Y + ($live.T + ($liveIdx + 0.5) * ($live.content_h / $live.count)) * $scale)
          [Win]::SetCursorPos($liveX, $liveY) | Out-Null
          Start-Sleep -Milliseconds 30
          [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
          Start-Sleep -Milliseconds 20
          [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
          Start-Sleep -Milliseconds 70
          continue
        }
      }
      # Fallback: initial geometry.
      [Win]::SetCursorPos($treeCenterX, (Row-Y $idx)) | Out-Null
      Start-Sleep -Milliseconds 30
      [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 20
      [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 70
    }
    Start-Sleep -Seconds 2

    $selectsAfter = (Select-String -Path $LogPath -Pattern "tree-select").Count
    $opensAfter = (Select-String -Path $LogPath -Pattern "tree-open").Count
    $fileTabsAfter = (Select-String -Path $LogPath -Pattern "surface-new kind=file").Count
    $rootLoadsAfter = (Select-String -Path $LogPath -Pattern 'tree-load dir=""').Count
    $lastSelect = Get-LastTrace "tree-select"
    $lastOpen = Get-LastTrace "tree-open"

    Write-Output ("file clicks: {0}; selects +{1}; opens +{2}; file tabs +{3}" -f `
      $targets.Count, ($selectsAfter - $selectsBefore), ($opensAfter - $opensBefore), `
      ($fileTabsAfter - $fileTabsBefore))
    Write-Output "last select: $lastSelect"
    Write-Output "last open: $lastOpen"

    if (($selectsAfter - $selectsBefore) -ne $targets.Count) {
      $failures += ("selection count {0} != clicks {1}" -f ($selectsAfter - $selectsBefore), $targets.Count)
    }
    if (($opensAfter - $opensBefore) -ne $targets.Count) {
      $failures += ("opens {0} != clicks {1}" -f ($opensAfter - $opensBefore), $targets.Count)
    }
    if (($fileTabsAfter - $fileTabsBefore) -ne $targets.Count) {
      $failures += ("file tabs +{0} != clicks {1}" -f ($fileTabsAfter - $fileTabsBefore), $targets.Count)
    }
    if (($rootLoadsAfter - $rootLoadsBefore) -ne 0) {
      $failures += ("root reloaded {0}x during tab opens" -f ($rootLoadsAfter - $rootLoadsBefore))
    }
    if ((Select-String -Path $LogPath -Pattern "surface-new kind=files").Count -gt 1) {
      $failures += "more than one files browser surface created"
    }
    $selectPath = if ($lastSelect -match 'path="([^"]*)"') { $Matches[1] } else { "" }
    $openPath = if ($lastOpen -match 'path="([^"]*)"') { $Matches[1] } else { "" }
    if ($selectPath -and $openPath -and ($selectPath -ne $openPath)) {
      $failures += ("last select {0} != last open {1}" -f $selectPath, $openPath)
    }

    # --- Return to the raw Files tab: the tree should expand back to the pane
    $tabsLine = Get-LastTrace "bounds files-tab"
    if ($tabsLine) {
      $tab = Parse-Numbers $tabsLine
      $tabX = [int]($origin.X + (($tab.L + $tab.R) / 2.0) * $scale)
      $tabY = [int]($origin.Y + (($tab.T + $tab.B) / 2.0) * $scale)
      $expandBefore = (Select-String -Path $LogPath -Pattern "cover-expand").Count
      $coverPreview = (Get-LastTrace "bounds preview-body")
      Click-At $tabX $tabY
      Start-Sleep -Milliseconds 300
      $expandAfter = (Select-String -Path $LogPath -Pattern "cover-expand").Count
      # The file content must stay visible (frozen bounds) during the cover:
      # it gets covered by the growing tree, not vanished by the swap.
      $coverFrames = (Select-String -Path $LogPath -Pattern "split-frame").Count
      $settledPreview = (Get-LastTrace "bounds preview-body")
      if ($coverPreview -and $settledPreview -and ($coverPreview -ne $settledPreview)) {
        $failures += "preview bounds changed across the cover transition"
      }
      Write-Output ("raw return: expands +{0}" -f ($expandAfter - $expandBefore))
      Start-Sleep -Milliseconds 400
      $rawLine = Get-LastTrace "tree-rows"
      if ($rawLine) {
        $raw = Parse-Numbers $rawLine
        Write-Output ("raw tree: L={0} R={1}" -f $raw.L, $raw.R)
        if (($expandAfter - $expandBefore) -lt 1) {
          $failures += "returning to the Files tab did not expand the tree"
        }
        if (($raw.R - $raw.L) -lt 400) {
          $failures += ("raw tree width {0} did not expand to full pane" -f [int]($raw.R - $raw.L))
        }
      }
    } else {
      Write-Output "no files-tab probe; skipping raw return"
    }
  }

  Write-Output "---- RESULT ----"
  if ($failures.Count -eq 0) {
    Write-Output "PASS ($Scenario)"
    exit 0
  } else {
    $failures | ForEach-Object { Write-Output "FAIL: $_" }
    exit 1
  }
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
