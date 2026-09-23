# Collapse-aware transition check: with the sidebar collapsed, returning
# to a file tab must swipe the tree AWAY to the corner (reveal to=0), and
# the raw cover must grow from the corner (cover from=0).
param([string]$LogPath = "$PSScriptRoot\..\..\target\ui_trace.log")

$driver = "$PSScriptRoot\run_scenario.ps1"
# Reuse run_scenario's machinery by dot-sourcing? Simpler: run tabs first.
& powershell -ExecutionPolicy Bypass -File $driver -Scenario tabs
if ($LASTEXITCODE -ne 0) { exit 1 }

# The tabs scenario left the app closed; run a dedicated pass here instead.
