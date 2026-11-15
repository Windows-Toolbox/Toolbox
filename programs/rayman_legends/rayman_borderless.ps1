# Makes the Rayman Legends window borderless and sets it to 2560x1440.
# Requirement: the game runs in windowed mode at 2560x1440.
# Can be started before or after the game (waits up to 5 minutes for it).
# Started by the toolbox page (Play button); the position is chosen there:
#   -Pos Center  -> centered (default)
#   -Pos Left    -> at the left edge
#   -Pos Right   -> at the right edge
# While the game runs the script keeps reading the position from data\rayman_legends.json (the toolbox page saves it
# there), so changing it on the page moves the window right away.
param(
  [ValidateSet('Center','Left','Right')] [string]$Pos = 'Center',
  [int]$Width = 2560,
  [int]$Height = 1440
)

Add-Type @'
using System; using System.Runtime.InteropServices;
public static class RB {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern IntPtr SetWindowLongPtr(IntPtr h, int i, IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
'@
# Use physical pixels (ignore Windows scaling, e.g. 125 %)
[RB]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

# Wait for the game window
$deadline = (Get-Date).AddMinutes(5)
do {
  $p = Get-Process -Name 'Rayman Legends' -ErrorAction SilentlyContinue | Where-Object MainWindowHandle -ne 0 | Select-Object -First 1
  if ($p) { break }
  Start-Sleep -Seconds 1
} while ((Get-Date) -lt $deadline)
if (-not $p) { exit 1 }
Start-Sleep -Seconds 3   # give the game time to finish building its window
$h = $p.MainWindowHandle

$screenW = [RB]::GetSystemMetrics(0)   # SM_CXSCREEN
function Get-X { switch ($script:Pos) { 'Left' { 0 } 'Right' { $screenW - $Width } default { [int](($screenW - $Width) / 2) } } }

# Position from the toolbox page – read again only when the file changed
$SettingsFile = Join-Path $PSScriptRoot '..\..\data\rayman_legends.json'
$settingsTime = $null
function Update-Pos {
  <# $true if the position on the page changed since the last look. #>
  try {
    $time = (Get-Item -LiteralPath $SettingsFile -ErrorAction Stop).LastWriteTimeUtc
    if ($time -eq $script:settingsTime) { return $false }
    $value = (Get-Content -Raw -Encoding UTF8 -LiteralPath $SettingsFile -ErrorAction Stop | ConvertFrom-Json).Pos
    $script:settingsTime = $time          # only now: a file caught half-written is simply read again next time
    if ($value -in 'Left', 'Center', 'Right' -and $value -ne $script:Pos) { $script:Pos = $value; return $true }
  } catch {}
  return $false
}
[void](Update-Pos)
$x = Get-X

$FRAME = [Int64]0x00CF0000   # WS_CAPTION, WS_THICKFRAME, WS_SYSMENU, min/max buttons
function Set-Borderless {
  $style = [Int64][RB]::GetWindowLongPtr($h, -16)
  [RB]::SetWindowLongPtr($h, -16, [IntPtr]($style -band (-bnot $FRAME))) | Out-Null
  # SWP_FRAMECHANGED|SWP_SHOWWINDOW|SWP_NOACTIVATE|SWP_NOZORDER
  [RB]::SetWindowPos($h, [IntPtr]::Zero, $x, 0, $Width, $Height, 0x0020 -bor 0x0040 -bor 0x0010 -bor 0x0004) | Out-Null
}
Set-Borderless

# As long as the game runs:
#  - "always on top" (above the taskbar) only while it is active
#  - remove the frame again if the game brings it back (e.g. after changing options)
# SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE = 0x13
$top = $false
while (-not $p.HasExited) {
  if (Update-Pos) { $x = Get-X; Set-Borderless }       # position changed on the toolbox page
  if (([Int64][RB]::GetWindowLongPtr($h, -16) -band 0x00C00000) -ne 0) { Set-Borderless }
  $active = ([RB]::GetForegroundWindow() -eq $h)
  if ($active -and -not $top) {
    [RB]::SetWindowPos($h, [IntPtr](-1), 0, 0, 0, 0, 0x13) | Out-Null; $top = $true   # HWND_TOPMOST
  } elseif (-not $active -and $top) {
    [RB]::SetWindowPos($h, [IntPtr](-2), 0, 0, 0, 0, 0x13) | Out-Null; $top = $false  # HWND_NOTOPMOST
  }
  Start-Sleep -Milliseconds 300
  $p.Refresh()
}
