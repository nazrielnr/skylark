param([string]$ShotPath = "$PSScriptRoot\..\..\target\boot_state.png")
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$exe = "$PSScriptRoot\..\..\target\debug\skylark.exe"
$log = "$PSScriptRoot\..\..\target\ui_trace.log"
if (Test-Path $log) { Remove-Item $log }
$env:SKYLARK_UI_TRACE = "1"
$proc = Start-Process -FilePath $exe -WorkingDirectory "$PSScriptRoot\..\.." -RedirectStandardError $log -RedirectStandardOutput "$PSScriptRoot\..\..\target\boot.stdout.log" -PassThru
Start-Sleep -Seconds 25
$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bmp.Save($ShotPath, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "shot: $ShotPath"
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
