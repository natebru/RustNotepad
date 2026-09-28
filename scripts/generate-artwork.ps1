$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot
$resources = Join-Path $root 'resources'

# Shared vector geometry keeps the SVG, Windows icon and GitHub artwork in sync.
$shapes = @(
    @{ box = @(16, 16, 480, 480, 104); color = '#303841' }
    @{ box = @(106, 104, 292, 330, 28); color = '#222A33' }
    @{ box = @(96, 78, 292, 338, 28); color = '#CE7959' }
    @{ box = @(128, 78, 260, 338, 28); color = '#F2EEE5' }
    @{ points = @(334, 78, 388, 132, 334, 132); color = '#C9C8C2' }
    @{ box = @(164, 151, 140, 16, 8); color = '#56616B' }
    @{ box = @(164, 203, 174, 16, 8); color = '#56616B' }
    @{ box = @(164, 255, 116, 16, 8); color = '#56616B' }
    @{ box = @(164, 307, 72, 16, 8); color = '#56616B' }
    @{ points = @(250, 371, 365, 256, 411, 302, 296, 417, 240, 427); color = '#303841' }
    @{ points = @(264, 372, 377, 259, 413, 295, 300, 408); color = '#E39871' }
    @{ points = @(282, 390, 395, 277, 413, 295, 300, 408); color = '#C66A48' }
    @{ points = @(264, 372, 300, 408, 252, 420); color = '#F2EEE5' }
    @{ points = @(256, 402, 270, 416, 252, 420); color = '#56616B' }
    @{ points = @(377, 259, 391, 245, 427, 281, 413, 295); color = '#8BAE84' }
)

function Draw-Mark([Drawing.Graphics]$Graphics) {
    foreach ($shape in $shapes) {
        $brush = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($shape.color))
        $path = [Drawing.Drawing2D.GraphicsPath]::new()
        try {
            if ($shape.box) {
                $x, $y, $w, $h, $r = $shape.box
                $d = 2 * $r
                $path.AddArc($x, $y, $d, $d, 180, 90)
                $path.AddArc(($x + $w - $d), $y, $d, $d, 270, 90)
                $path.AddArc(($x + $w - $d), ($y + $h - $d), $d, $d, 0, 90)
                $path.AddArc($x, ($y + $h - $d), $d, $d, 90, 90)
                $path.CloseFigure()
            } else {
                $points = for ($i = 0; $i -lt $shape.points.Count; $i += 2) {
                    [Drawing.PointF]::new($shape.points[$i], $shape.points[$i + 1])
                }
                $path.AddPolygon([Drawing.PointF[]]$points)
            }
            $Graphics.FillPath($brush, $path)
        } finally {
            $path.Dispose()
            $brush.Dispose()
        }
    }
}

function New-Mark([int]$Size) {
    $bitmap = [Drawing.Bitmap]::new($Size, $Size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.ScaleTransform(($Size / 512.0), ($Size / 512.0))
        Draw-Mark $graphics
    } finally { $graphics.Dispose() }
    return $bitmap
}

$svg = [Collections.Generic.List[string]]::new()
$svg.Add('<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512" role="img" aria-labelledby="title">')
$svg.Add('<title id="title">Rust Notepad - copper notebook and pencil on slate</title>')
foreach ($shape in $shapes) {
    if ($shape.box) {
        $x, $y, $w, $h, $r = $shape.box
        $svg.Add("<rect x=`"$x`" y=`"$y`" width=`"$w`" height=`"$h`" rx=`"$r`" fill=`"$($shape.color)`"/>")
    } else {
        $points = for ($i = 0; $i -lt $shape.points.Count; $i += 2) {
            "$($shape.points[$i]),$($shape.points[$i + 1])"
        }
        $svg.Add("<polygon points=`"$($points -join ' ')`" fill=`"$($shape.color)`"/>")
    }
}
$svg.Add('</svg>')
[IO.File]::WriteAllLines((Join-Path $resources 'icon.svg'), $svg)

$master = New-Mark 1024
try {
    $master.Save((Join-Path $resources 'icon.png'), [Drawing.Imaging.ImageFormat]::Png)
    $sizes = @(16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
    $frames = foreach ($size in $sizes) {
        $image = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $g = [Drawing.Graphics]::FromImage($image)
        $stream = [IO.MemoryStream]::new()
        try {
            $g.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $g.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $g.DrawImage($master, [Drawing.Rectangle]::new(0, 0, $size, $size))
            $image.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
            ,$stream.ToArray()
        } finally { $g.Dispose(); $image.Dispose(); $stream.Dispose() }
    }
    $file = [IO.File]::Create((Join-Path $resources 'app.ico'))
    $writer = [IO.BinaryWriter]::new($file)
    try {
        $writer.Write([uint16]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]$sizes.Count)
        $offset = 6 + 16 * $sizes.Count
        for ($i = 0; $i -lt $sizes.Count; $i++) {
            $dimension = $sizes[$i] % 256
            $writer.Write([byte]$dimension)
            $writer.Write([byte]$dimension)
            $writer.Write([uint16]0)
            $writer.Write([uint16]1)
            $writer.Write([uint16]32)
            $writer.Write([uint32]$frames[$i].Length)
            $writer.Write([uint32]$offset)
            $offset += $frames[$i].Length
        }
        foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
    } finally { $writer.Dispose() }
} finally { $master.Dispose() }

$card = [Drawing.Bitmap]::new(1280, 640)
$graphics = [Drawing.Graphics]::FromImage($card)
$brush = [Drawing.SolidBrush]::new([Drawing.Color]::White)
$fonts = @(
    [Drawing.Font]::new('Segoe UI', 68, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel),
    [Drawing.Font]::new('Segoe UI', 30, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel),
    [Drawing.Font]::new('Consolas', 20, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
)
try {
    $graphics.Clear([Drawing.ColorTranslator]::FromHtml('#222A33'))
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.TextRenderingHint = [Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $graphics.TranslateTransform(65, 120)
    $graphics.ScaleTransform(0.78, 0.78)
    Draw-Mark $graphics
    $graphics.ResetTransform()
    $brush.Color = [Drawing.ColorTranslator]::FromHtml('#8BAE84')
    $graphics.FillRectangle($brush, 526, 167, 48, 5)
    $graphics.DrawString('NATIVE WINDOWS / WRITTEN IN RUST', $fonts[2], $brush, 526, 195)
    $brush.Color = [Drawing.ColorTranslator]::FromHtml('#F2EEE5')
    $graphics.DrawString('Rust Notepad', $fonts[0], $brush, 519, 237)
    $brush.Color = [Drawing.ColorTranslator]::FromHtml('#BCC7D3')
    $graphics.DrawString('Just you and your words.', $fonts[1], $brush, 524, 336)
    $brush.Color = [Drawing.ColorTranslator]::FromHtml('#E39871')
    $graphics.DrawString('NO AI. NO TELEMETRY. JUST TEXT.', $fonts[2], $brush, 526, 415)
    $card.Save((Join-Path $resources 'social-preview.png'), [Drawing.Imaging.ImageFormat]::Png)
} finally {
    foreach ($font in $fonts) { $font.Dispose() }
    $brush.Dispose(); $graphics.Dispose(); $card.Dispose()
}
Write-Output 'Generated resources\icon.svg, icon.png, app.ico and social-preview.png'
