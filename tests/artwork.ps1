param([Parameter(Mandatory)][string]$Executable)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
if (-not ('NotepadIconTest' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NotepadIconTest {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr LoadLibraryEx(string path, IntPtr file, uint flags);
    [DllImport("kernel32.dll")] public static extern bool FreeLibrary(IntPtr module);
    [DllImport("user32.dll", EntryPoint="LoadImageW", SetLastError=true)]
    public static extern IntPtr LoadImage(IntPtr module, IntPtr id, uint type, int x, int y, uint flags);
    [DllImport("user32.dll")] public static extern bool DestroyIcon(IntPtr icon);
}
'@
}
$root = Split-Path $PSScriptRoot
$sizes = @(16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
$bytes = [IO.File]::ReadAllBytes((Join-Path $root 'resources\app.ico'))
if ([BitConverter]::ToUInt16($bytes, 0) -ne 0 -or
    [BitConverter]::ToUInt16($bytes, 2) -ne 1 -or
    [BitConverter]::ToUInt16($bytes, 4) -ne $sizes.Count) { throw 'Invalid ICO header' }
$module = [NotepadIconTest]::LoadLibraryEx((Resolve-Path $Executable).Path, [IntPtr]::Zero, 0x22)
if ($module -eq [IntPtr]::Zero) { throw 'Cannot load executable icon resources' }
try {
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $size = $sizes[$i]
        $entry = 6 + 16 * $i
        if ($bytes[$entry] -ne ($size % 256) -or $bytes[$entry + 1] -ne ($size % 256)) {
            throw "ICO size entry mismatch: $size"
        }
        $length = [BitConverter]::ToUInt32($bytes, $entry + 8)
        $offset = [BitConverter]::ToUInt32($bytes, $entry + 12)
        if ($offset + $length -gt $bytes.Length) { throw 'Truncated ICO frame' }
        $stream = [IO.MemoryStream]::new($bytes, $offset, $length)
        $frame = [Drawing.Bitmap]::new($stream)
        $handle = [IntPtr]::Zero
        $icon = $null
        $embedded = $null
        try {
            if ($frame.Width -ne $size -or $frame.Height -ne $size) { throw "Wrong frame size: $size" }
            if ($frame.GetPixel(0, 0).A -ne 0) { throw "Icon corner is not transparent: $size" }
            $handle = [NotepadIconTest]::LoadImage($module, [IntPtr]1, 1, $size, $size, 0)
            if ($handle -eq [IntPtr]::Zero) { throw "Embedded icon cannot load at $size pixels" }
            $icon = [Drawing.Icon]::FromHandle($handle)
            if ($icon.Width -ne $size -or $icon.Height -ne $size) { throw "Wrong embedded size: $size" }
            $embedded = $icon.ToBitmap()
            $center = [int]($size / 2)
            if ($embedded.GetPixel($center, $center).ToArgb() -ne $frame.GetPixel($center, $center).ToArgb()) {
                throw "Executable icon differs from source at $size pixels"
            }
        } finally {
            if ($embedded) { $embedded.Dispose() }
            if ($icon) { $icon.Dispose() }
            if ($handle -ne [IntPtr]::Zero) { [void][NotepadIconTest]::DestroyIcon($handle) }
            $frame.Dispose(); $stream.Dispose()
        }
    }
} finally { [void][NotepadIconTest]::FreeLibrary($module) }
foreach ($image in @(@('icon.png', 1024, 1024), @('social-preview.png', 1280, 640))) {
    $bitmap = [Drawing.Bitmap]::new((Join-Path $root "resources\$($image[0])"))
    try {
        if ($bitmap.Width -ne $image[1] -or $bitmap.Height -ne $image[2]) { throw "Wrong dimensions: $($image[0])" }
    } finally { $bitmap.Dispose() }
}
Write-Output 'PASS: ten transparent ICO sizes, executable icon resources, PNG branding dimensions'
