#requires -Version 7.0
<#
.SYNOPSIS
    Замеры памяти и времени CLI-конвейера: таблица «до/после» для этапов плана.

.DESCRIPTION
    Каждый сценарий запускается $Runs раз, берётся медиана. Память читается из
    счётчиков процесса (PeakWorkingSet64 / PeakPagedMemorySize64), поэтому это
    пик ОС, а не оценка «на глаз». Вывод — markdown-таблица, которую можно
    вставлять в docs/performance.md.

    -Baseline  печатает таблицу и ничего не проверяет;
    -Check     сравнивает медианы с порогами из $Thresholds и падает (exit 1)
               при регрессии больше 10%.

    Пороги обновляются осознанно: при улучшении — уменьшить, при обоснованном
    ухудшении — увеличить и записать причину в optimaze.md.

.PARAMETER Runs
    Сколько раз запускать каждый сценарий (по умолчанию 3).

.PARAMETER Check
    Проверить пороги и вернуть exit code 1 при регрессии.

.PARAMETER Baseline
    Только напечатать таблицу (режим по умолчанию, если не указан -Check).

.EXAMPLE
    pwsh -File scripts/bench.ps1 -Baseline

.EXAMPLE
    pwsh -File scripts/bench.ps1 -Check
#>
[CmdletBinding()]
param(
    [int]$Runs = 3,
    [switch]$Check,
    [switch]$Baseline
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target/release/pixel-image.exe'
$benchDir = Join-Path $root 'out/bench'

if (-not (Test-Path $exe)) {
    throw "нет $exe — соберите: cargo build --release"
}
if (-not (Test-Path $benchDir)) {
    New-Item -ItemType Directory -Force -Path $benchDir | Out-Null
}

$big = Join-Path $benchDir 'photo-24mp.jpg'
$mid = Join-Path $benchDir 'photo-12mp.jpg'
$alpha = Join-Path $benchDir 'cutout-alpha.png'
$small = Join-Path $benchDir 'small.jpg'
foreach ($fixture in @($big, $mid, $alpha, $small)) {
    if (-not (Test-Path $fixture)) {
        throw "нет фикстуры $fixture — запустите: pwsh -File scripts/make-fixtures.ps1"
    }
}

# Пороги — медианы этапа 0 (см. optimaze.md, §2). Проверка в -Check добавляет
# к ним ещё 10%, поэтому эффективный запас ~10% по памяти и ~25% по времени.
# Память: WsMb/CommitMb — пик процесса. Время: Ms — общая длительность запуска.
$Thresholds = [ordered]@{
    'pixel-art 24mp'     = @{ WsMb = 196.4; CommitMb = 193.9; Ms = 630 }
    'pixel-art 24mp box' = @{ WsMb = 165.7; CommitMb = 163.1; Ms = 160 }
    'cutout 24mp all'    = @{ WsMb = 231.1; CommitMb = 329.9; Ms = 410 }
    'cutout alpha png'   = @{ WsMb = 53.0;  CommitMb = 52.9;  Ms = 70 }
    'pixel-art 12mp'     = @{ WsMb = 102.6; CommitMb = 99.6;  Ms = 340 }
    'pixel-art small'    = @{ WsMb = 47.3;  CommitMb = 43.7;  Ms = 70 }
}

$Scenarios = @(
    @{ Name = 'pixel-art 24mp';     Args = @($big) }
    @{ Name = 'pixel-art 24mp box'; Args = @($big, '-f', 'box') }
    @{ Name = 'cutout 24mp all';    Args = @($big, '--cutbg', '--thin-gaps', '--remove-holes') }
    @{ Name = 'cutout alpha png';   Args = @($alpha, '--cutbg') }
    @{ Name = 'pixel-art 12mp';     Args = @($mid) }
    @{ Name = 'pixel-art small';    Args = @($small) }
)

function Invoke-Once {
    param([string[]]$ArgumentList)

    $outFile = Join-Path $benchDir 'stdout.txt'
    $errFile = Join-Path $benchDir 'stderr.txt'
    Remove-Item $outFile, $errFile -ErrorAction SilentlyContinue

    # System.Diagnostics.Process напрямую: Start-Process -PassThru в этой среде
    # отдаёт ExitCode = null, а нам нужен и код возврата, и счётчики памяти.
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $exe
    foreach ($arg in $ArgumentList) { $info.ArgumentList.Add($arg) }
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.UseShellExecute = $false
    $info.WorkingDirectory = $root

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $proc = [System.Diagnostics.Process]::Start($info)
    # Читаем потоки асинхронно, иначе буфер пайпа переполнится и процесс встанет.
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()

    $ws = 0L
    $cm = 0L
    while (-not $proc.HasExited) {
        try {
            $proc.Refresh()
            if ($proc.PeakWorkingSet64 -gt $ws) { $ws = $proc.PeakWorkingSet64 }
            if ($proc.PeakPagedMemorySize64 -gt $cm) { $cm = $proc.PeakPagedMemorySize64 }
        } catch {
            # процесс уже завершился между Refresh и чтением — не ошибка
        }
        Start-Sleep -Milliseconds 5
    }
    $proc.WaitForExit()
    $sw.Stop()
    # После выхода процесса Refresh() бросает исключение, а Peak* без обновления
    # может вернуть 0 — поэтому пик берём только из «живых» замеров выше.

    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    Set-Content -Path $outFile -Value $stdout -Encoding utf8
    Set-Content -Path $errFile -Value $stderr -Encoding utf8

    $internal = $null
    $line = ($stdout -split "`r?`n") | Where-Object { $_ -like 'Время:*' } | Select-Object -Last 1
    if ($line) { $internal = $line }

    [pscustomobject]@{
        ExitMs    = [int]$sw.ElapsedMilliseconds
        WsMb      = [math]::Round($ws / 1MB, 1)
        CommitMb  = [math]::Round($cm / 1MB, 1)
        Internal  = $internal
        ExitCode  = $proc.ExitCode
    }
}

function Get-Median {
    param([double[]]$Values)
    $sorted = $Values | Sort-Object
    $count = $sorted.Count
    if ($count -eq 0) { return 0 }
    if ($count % 2 -eq 1) { return $sorted[[int](($count - 1) / 2)] }
    return [math]::Round(($sorted[$count / 2 - 1] + $sorted[$count / 2]) / 2, 1)
}

$failed = $false
$rows = foreach ($scenario in $Scenarios) {
    # Имя не должно совпадать с параметром $Runs: переменные в PowerShell
    # регистронезависимы, иначе счётчик прогонов затирается результатами.
    $samples = for ($i = 0; $i -lt $Runs; $i++) {
        $result = Invoke-Once -ArgumentList $scenario.Args
        if ($result.ExitCode -ne 0) {
            throw "сценарий '$($scenario.Name)' завершился с кодом $($result.ExitCode), см. $benchDir/stderr.txt"
        }
        $result
    }

    $median = [pscustomobject]@{
        Name     = $scenario.Name
        Ms       = [int](Get-Median -Values ($samples.ExitMs))
        WsMb     = Get-Median -Values ($samples.WsMb)
        CommitMb = Get-Median -Values ($samples.CommitMb)
        Internal = ($samples | Select-Object -Last 1).Internal
    }

    $limit = $Thresholds[$scenario.Name]
    $verdict = ''
    if ($Check -and $limit) {
        $tolerance = 1.10
        $problems = @()
        if ($median.WsMb -gt $limit.WsMb * $tolerance) {
            $problems += "WS $($median.WsMb) МБ > $($limit.WsMb) МБ"
        }
        if ($median.CommitMb -gt $limit.CommitMb * $tolerance) {
            $problems += "commit $($median.CommitMb) МБ > $($limit.CommitMb) МБ"
        }
        if ($median.Ms -gt $limit.Ms * $tolerance) {
            $problems += "время $($median.Ms) мс > $($limit.Ms) мс"
        }
        if ($problems.Count -gt 0) {
            $failed = $true
            $verdict = 'РЕГРЕССИЯ: ' + ($problems -join '; ')
        } else {
            $verdict = 'ok'
        }
    }

    [pscustomobject]@{
        Сценарий = $median.Name
        Время    = $median.Ms
        WS       = $median.WsMb
        Commit   = $median.CommitMb
        Внутри   = $median.Internal
        Итог     = $verdict
    }
}

$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

Write-Host ''
Write-Host '| Сценарий | Время, мс | peak WS, МБ | peak commit, МБ |'
Write-Host '| --- | --- | --- | --- |'
foreach ($row in $rows) {
    Write-Host ("| {0} | {1} | {2} | {3} |" -f $row.Сценарий, $row.Время, $row.WS, $row.Commit)
}

if ($Check) {
    if ($failed) {
        Write-Host ''
        Write-Host 'Есть регрессии — см. колонку «Итог».' -ForegroundColor Red
        exit 1
    }
    Write-Host ''
    Write-Host 'Все пороги в норме.' -ForegroundColor Green
}
