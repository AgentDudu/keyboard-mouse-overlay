# Capture a short animated demo of the overlay for the repo docs.
#
# Needs ffmpeg on PATH. Sends tagged-free synthetic F-key and modifier input (nothing a real app
# binds, so nothing else on the desktop reacts) while grabbing a screen region, then encodes a
# palette-optimised GIF. Keys are released in `finally`, so it cannot leave modifiers stuck.
#
#   powershell -File tools/capture-gif.ps1 -Theme light -Out docs/phase3-demo-light.gif

param(
    [string]$Exe = "target\release\kmoverlay.exe",
    [string]$Out = "docs\phase3-demo.gif",
    [string]$Theme = "dark",
    # Size factor handed to the overlay; 1.5 shows what a 150 % display looks like.
    [double]$Scale = 0,
    [double]$Seconds = 3.0,
    [int]$Fps = 12,
    # Region to grab: x,y,width,height in physical pixels, around the bottom-centre strip.
    [string]$Region = "340,924,1240,112",
    # A plain window behind the overlay, so the demo shows the strip and not whatever is open.
    [string]$Backdrop = '24,26,32'
)

$ErrorActionPreference = 'Stop'
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw 'ffmpeg not on PATH' }
if (-not (Test-Path $Exe)) { throw "not found: $Exe (cargo build --release first)" }

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public class K { [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, ulong extra); }
'@

$VK = @{ Ctrl = 0x11; Shift = 0x10; Alt = 0x12; F13 = 0x7C; F14 = 0x7D; F15 = 0x7E; F16 = 0x7F; F17 = 0x80 }
function Press([string]$name) { [K]::keybd_event($VK[$name], 0, 0, 0) }
function Release([string]$name) { [K]::keybd_event($VK[$name], 0, 2, 0) }

# (at seconds, key, down)
$timeline = @(
    @(0.05, 'Ctrl', $true), @(0.30, 'F13', $true), @(0.55, 'Shift', $true),
    @(0.90, 'F13', $false), @(1.05, 'F14', $true), @(1.30, 'F15', $true),
    @(1.55, 'Alt', $true), @(1.70, 'F14', $false), @(1.95, 'F15', $false),
    @(2.10, 'F16', $true), @(2.35, 'F17', $true), @(2.60, 'Ctrl', $false),
    @(2.75, 'Shift', $false), @(2.85, 'Alt', $false)
)

$x, $y, $w, $h = ($Region -split ',') | ForEach-Object { [int]$_ }
$frames = Join-Path $env:TEMP ("kmcap-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $frames | Out-Null
$outDir = Split-Path -Parent $Out
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }

$form = $null
if ($Backdrop) {
    $r, $g, $b = ($Backdrop -split ',') | ForEach-Object { [int]$_ }
    $form = New-Object System.Windows.Forms.Form
    $form.FormBorderStyle = 'None'
    $form.MaximizeBox = $false
    $form.ShowInTaskbar = $false
    $form.TopMost = $false
    $form.Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $form.BackColor = [System.Drawing.Color]::FromArgb($r, $g, $b)
    $form.Show()
    $form.Activate()
    Start-Sleep -Milliseconds 300
}

$overlayArgs = @('--overlay', '--show-injected', '--theme', $Theme, '--duration', '9000')
if ($Scale -gt 0) { $overlayArgs += @('--scale', "$Scale") }
$process = Start-Process -FilePath (Resolve-Path $Exe) -ArgumentList $overlayArgs -PassThru
Start-Sleep -Milliseconds 800

$rect = New-Object System.Drawing.Rectangle $x, $y, $w, $h
$count = [math]::Max(1, [int]($Seconds * $Fps))
$watch = [System.Diagnostics.Stopwatch]::StartNew()
$next = 0
$written = 0
try {
    for ($frame = 0; $frame -lt $count; $frame++) {
        $due = $watch.Elapsed.TotalSeconds
        while ($next -lt $timeline.Count -and $timeline[$next][0] -le $due) {
            $item = $timeline[$next]
            if ($item[2]) { Press $item[1] } else { Release $item[1] }
            $next++
        }
        [System.Windows.Forms.Application]::DoEvents()
        $bitmap = New-Object System.Drawing.Bitmap $w, $h
        $canvas = [System.Drawing.Graphics]::FromImage($bitmap)
        $canvas.CopyFromScreen($x, $y, 0, 0, $bitmap.Size)
        $bitmap.Save((Join-Path $frames ("f{0:d3}.png" -f $frame)), [System.Drawing.Imaging.ImageFormat]::Png)
        $canvas.Dispose(); $bitmap.Dispose()
        $written++
        $target = ($frame + 1) / $Fps
        $sleep = [int](($target - $watch.Elapsed.TotalSeconds) * 1000)
        if ($sleep -gt 0) { Start-Sleep -Milliseconds $sleep }
    }
}
finally {
    foreach ($name in 'F13', 'F14', 'F15', 'F16', 'F17', 'Ctrl', 'Shift', 'Alt') { Release $name }
    if ($form) { $form.Close() }
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 200
}

# One ffmpeg pass: split, build a palette from one copy, apply it to the other.
& ffmpeg -hide_banner -loglevel error -y -framerate $Fps -i (Join-Path $frames 'f%03d.png') `
    -vf "split[s][o];[o]palettegen=reserve_transparent=0[p];[s][p]paletteuse=dither=bayer:bayer_scale=3" `
    -loop 0 $Out
$code = $LASTEXITCODE
Remove-Item -Recurse -Force $frames
if ($code -ne 0) { throw "ffmpeg failed ($code) after writing $written frames" }
"{0} frames -> {1} ({2:n0} kB)" -f $written, $Out, [math]::Round((Get-Item $Out).Length / 1kb)
