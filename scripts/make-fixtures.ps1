#requires -Version 7.0
<#
.SYNOPSIS
    Генерирует детерминированные фикстуры для замеров памяти и времени.

.DESCRIPTION
    Фикстуры не хранятся в репозитории: они собираются из кода (System.Drawing)
    с фиксированным seed, поэтому одинаковы от запуска к запуску. Складываются в
    out/bench/ (каталог в .gitignore).

    - photo-24mp.jpg  — 6000x4000, «фото-подобная» картинка с шумом и фоном;
    - photo-12mp.jpg  — 4000x3000, типичный кадр с телефона;
    - cutout-alpha.png — 1024x1536 с настоящей прозрачностью вокруг объекта.

.PARAMETER Force
    Перезаписать существующие файлы.

.EXAMPLE
    pwsh -File scripts/make-fixtures.ps1
#>
[CmdletBinding()]
param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$dir = Join-Path $root 'out/bench'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

function New-PhotoLike {
    param(
        [int]$Width,
        [int]$Height,
        [int]$Seed,
        [int]$Shapes
    )

    $bmp = New-Object System.Drawing.Bitmap $Width, $Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::FromArgb(255, 242, 238, 226))
    $rnd = New-Object System.Random $Seed
    for ($i = 0; $i -lt $Shapes; $i++) {
        $color = [System.Drawing.Color]::FromArgb(
            255,
            $rnd.Next(0, 256),
            $rnd.Next(0, 256),
            $rnd.Next(0, 256))
        $brush = New-Object System.Drawing.SolidBrush $color
        $x = $rnd.Next(0, $Width)
        $y = $rnd.Next(0, $Height)
        $w = $rnd.Next($Width / 20, $Width / 4)
        $h = $rnd.Next($Height / 20, $Height / 4)
        $g.FillEllipse($brush, $x, $y, $w, $h)
        $brush.Dispose()
    }
    $g.Dispose()
    return $bmp
}

function New-AlphaCutout {
    param(
        [int]$Width,
        [int]$Height
    )

    $bmp = New-Object System.Drawing.Bitmap $Width, $Height, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::FromArgb(0, 0, 0, 0))
    $body = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 60, 90, 150))
    $g.FillEllipse($body, $Width * 0.2, $Height * 0.1, $Width * 0.6, $Height * 0.85)
    $body.Dispose()
    $accent = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 230, 200, 90))
    $g.FillEllipse($accent, $Width * 0.35, $Height * 0.25, $Width * 0.3, $Height * 0.25)
    $accent.Dispose()
    $g.Dispose()
    return $bmp
}

function Save-Jpeg {
    param([System.Drawing.Bitmap]$Bitmap, [string]$Path)
    if (-not $Force -and (Test-Path $Path)) {
        Write-Host "уже есть: $Path"
        $Bitmap.Dispose()
        return
    }
    $Bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Jpeg)
    $Bitmap.Dispose()
    $size = [Math]::Round((Get-Item $Path).Length / 1MB, 2)
    Write-Host ("создано: {0} ({1} МБ)" -f $Path, $size)
}

function Save-Png {
    param([System.Drawing.Bitmap]$Bitmap, [string]$Path)
    if (-not $Force -and (Test-Path $Path)) {
        Write-Host "уже есть: $Path"
        $Bitmap.Dispose()
        return
    }
    $Bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $Bitmap.Dispose()
    $size = [Math]::Round((Get-Item $Path).Length / 1MB, 2)
    Write-Host ("создано: {0} ({1} МБ)" -f $Path, $size)
}

Save-Jpeg (New-PhotoLike -Width 6000 -Height 4000 -Seed 11 -Shapes 500) (Join-Path $dir 'photo-24mp.jpg')
Save-Jpeg (New-PhotoLike -Width 4000 -Height 3000 -Seed 23 -Shapes 320) (Join-Path $dir 'photo-12mp.jpg')
Save-Png (New-AlphaCutout -Width 1440 -Height 2160) (Join-Path $dir 'cutout-alpha.png')

Write-Host "фикстуры в $dir"
