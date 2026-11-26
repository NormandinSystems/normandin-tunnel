# Starts an installed copy of the desktop app. Passes once the web view has
# loaded the interface and the interface has reached the engine, and keeps a
# screenshot.
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][string]$Name
)
$env:TUNNEL_DATA_DIR = Join-Path $env:RUNNER_TEMP "data-$Name"
$env:TUNNEL_LOG = "info"
$log = Join-Path $env:RUNNER_TEMP "start-$Name.txt"
$app = Start-Process -FilePath $Exe -PassThru -RedirectStandardError $log
function Test-Ready { (Test-Path $log) -and [bool](Select-String -Path $log -Pattern "interface ready" -Quiet) }
$deadline = (Get-Date).AddSeconds(120)
while ((Get-Date) -lt $deadline -and -not (Test-Ready) -and -not $app.HasExited) {
  Start-Sleep -Seconds 2
}
Start-Sleep -Seconds 5
try {
  Add-Type -AssemblyName System.Windows.Forms, System.Drawing
  $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
  $bitmap.Save((Join-Path $env:RUNNER_TEMP "screen-$Name.png"))
} catch {
  Write-Host "No screenshot: $_"
}
if (Test-Path $log) { Get-Content $log }
if ($app.HasExited) {
  Write-Error "The app exited on its own with code $($app.ExitCode)"
  exit 1
}
Stop-Process -Id $app.Id -Force
if (-not (Test-Ready)) {
  Write-Error "The interface never reached the engine"
  exit 1
}
Write-Host "The app started and its interface reached the engine."
