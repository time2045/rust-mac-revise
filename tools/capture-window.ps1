param([Parameter(Mandatory=$true)][int]$ProcessId, [Parameter(Mandatory=$true)][string]$OutputPath)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class WindowCapture {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
}
'@
$process = Get-Process -Id $ProcessId
$window = $process.MainWindowHandle
if ($window -eq [IntPtr]::Zero) { throw 'Application window is unavailable' }
$rect = [WindowCapture+RECT]::new()
if (-not [WindowCapture]::GetWindowRect($window, [ref]$rect)) { throw 'Cannot read window bounds' }
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
$bitmap = [System.Drawing.Bitmap]::new($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
try { $captured = [WindowCapture]::PrintWindow($window, $dc, 2) }
finally { $graphics.ReleaseHdc($dc) }
if (-not $captured) { throw 'Window capture failed' }
$bitmap.Save([System.IO.Path]::GetFullPath($OutputPath), [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
$root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
$children = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
$names = @()
for ($i=0; $i -lt [Math]::Min($children.Count, 120); $i++) {
    if ($children.Item($i).Current.Name) { $names += $children.Item($i).Current.Name }
}
[PSCustomObject]@{ process_id=$ProcessId; title=$process.MainWindowTitle; width=$width; height=$height; controls=$names } | ConvertTo-Json -Depth 3
