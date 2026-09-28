param([switch]$Test)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw 'Install Visual Studio Build Tools with the C++ workload and Windows SDK first.' }
$visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $visualStudio) { throw 'No Visual Studio installation with x64 C++ build tools was found.' }
$setup = Join-Path $visualStudio 'VC\Auxiliary\Build\vcvarsall.bat'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = (Get-Command cargo -ErrorAction Stop).Source }
$env:PATH = (Split-Path $vswhere) + ';' + $env:PATH
Push-Location $root
try {
    $commands = @(
        "call `"$setup`" x64 >nul",
        "`"$cargo`" fmt --check",
        "`"$cargo`" clippy --locked --all-targets --target x86_64-pc-windows-msvc -- -D warnings",
        "`"$cargo`" test --locked --target x86_64-pc-windows-msvc",
        "`"$cargo`" build --locked --release --target x86_64-pc-windows-msvc"
    )
    & $env:ComSpec /c ($commands -join ' && ')
    if ($LASTEXITCODE -ne 0) { throw "Build failed with exit code $LASTEXITCODE" }
    $executable = Join-Path $root 'target\x86_64-pc-windows-msvc\release\notepad.exe'
    if ($Test) {
        $report = Join-Path $root 'target\native-self-test.txt'
        $process = Start-Process $executable -ArgumentList '--self-test', "`"$report`"" -PassThru
        if (-not $process.WaitForExit(120000)) {
            Stop-Process -Id $process.Id
            throw 'Native checks timed out.'
        }
        Get-Content $report
        if ($process.ExitCode -ne 0) { throw 'Native checks failed.' }
        & (Join-Path $root 'tests\smoke.ps1') -Executable $executable
    }
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'dist') | Out-Null
    Copy-Item $executable (Join-Path $root 'dist\notepad.exe')
    Write-Output "Portable executable: $(Join-Path $root 'dist\notepad.exe')"
} finally {
    Pop-Location
}
