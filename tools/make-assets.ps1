# Regenerates the embedded icon assets from assets/src (pear-desktop artwork, MIT).
# Output: raw RGBA buffers (no PNG decoder needed at runtime) + a multi-size app.ico.
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$src = Join-Path $root 'assets\src'
$out = Join-Path $root 'assets'

function Resize([string]$path, [int]$size) {
    $img = [System.Drawing.Image]::FromFile($path)
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($img, 0, 0, $size, $size)
    $g.Dispose(); $img.Dispose()
    return $bmp
}

function Write-Rgba([System.Drawing.Bitmap]$bmp, [string]$dest) {
    $rect = New-Object System.Drawing.Rectangle(0, 0, $bmp.Width, $bmp.Height)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $bytes = New-Object byte[] ($bmp.Width * $bmp.Height * 4)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
    $bmp.UnlockBits($data)
    for ($i = 0; $i -lt $bytes.Length; $i += 4) {
        # BGRA (GDI+) -> RGBA
        $b = $bytes[$i]; $bytes[$i] = $bytes[$i + 2]; $bytes[$i + 2] = $b
    }
    [System.IO.File]::WriteAllBytes($dest, $bytes)
}

function Get-PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

Write-Rgba (Resize "$src\icon.png" 64) "$out\icon64.rgba"
Write-Rgba (Resize "$src\tray.png" 32) "$out\tray32.rgba"
Write-Rgba (Resize "$src\tray-paused.png" 32) "$out\tray-paused32.rgba"

# ICO with PNG-compressed entries (supported since Windows Vista).
$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = New-Object 'System.Collections.Generic.List[byte[]]'
foreach ($s in $sizes) { $pngs.Add((Get-PngBytes (Resize "$src\icon.png" $s))) }
$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($ms)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$i].Length
}
foreach ($p in $pngs) { $w.Write($p) }
$w.Flush()
[System.IO.File]::WriteAllBytes("$out\app.ico", $ms.ToArray())
Get-ChildItem $out -File | Select-Object Name, Length
