param(
    [ValidateSet('x86', 'x64', 'arm64')][string]$Architecture = 'x64',
    [string]$ReleaseVersion,
    [switch]$Test,
    [switch]$SmokeTest,
    [switch]$Package
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$target = @{ x86 = 'i686-pc-windows-msvc'; x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Architecture]
$vcArchitecture = @{ x86 = 'x64_x86'; x64 = 'x64'; arm64 = 'x64_arm64' }[$Architecture]
if ($ReleaseVersion) {
    if ($ReleaseVersion -cnotmatch '^0\.1\.(\d{8})$') { throw 'ReleaseVersion must be 0.1.YYYYMMDD.' }
    $date = [DateTime]::ParseExact($Matches[1], 'yyyyMMdd', [Globalization.CultureInfo]::InvariantCulture)
    if ($date.Year -lt 2000) { throw 'Release date must be in or after 2000.' }
}
if ($Package -and -not $ReleaseVersion) { throw '-Package requires -ReleaseVersion.' }
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw 'Install Visual Studio Build Tools with the C++ workload and Windows SDK first.' }
$visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $visualStudio) { throw 'No Visual Studio installation with x64 C++ build tools was found.' }
$setup = Join-Path $visualStudio 'VC\Auxiliary\Build\vcvarsall.bat'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = (Get-Command cargo -ErrorAction Stop).Source }
$env:PATH = (Split-Path $vswhere) + ';' + $env:PATH
$previousVersion = $env:RUSTNOTEPAD_VERSION
Push-Location $root
try {
    if ($ReleaseVersion) { $env:RUSTNOTEPAD_VERSION = $ReleaseVersion }
    $commands = @(
        "call `"$setup`" $vcArchitecture >nul",
        "`"$cargo`" fmt --check",
        "`"$cargo`" clippy --locked --all-targets --target $target -- -D warnings",
        "`"$cargo`" test --locked --target $target",
        "`"$cargo`" build --locked --release --target $target"
    )
    & $env:ComSpec /c ($commands -join ' && ')
    if ($LASTEXITCODE -ne 0) { throw "Build failed with exit code $LASTEXITCODE" }
    $executable = Join-Path $root "target\$target\release\notepad.exe"
    $bytes = [IO.File]::ReadAllBytes($executable)
    $pe = [BitConverter]::ToInt32($bytes, 0x3c)
    if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x4550) { throw 'Missing PE signature.' }
    $expectedMachine = @{ x86 = 0x14c; x64 = 0x8664; arm64 = 0xaa64 }[$Architecture]
    if ([BitConverter]::ToUInt16($bytes, $pe + 4) -ne $expectedMachine) { throw "Executable is not $Architecture." }
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
    if ($ReleaseVersion -and ($info.ProductVersion -ne $ReleaseVersion -or $info.FileVersion -ne $ReleaseVersion)) {
        throw "Executable version does not match $ReleaseVersion."
    }
    if ($Test) {
        $report = Join-Path $root "target\$target\native-self-test.txt"
        $process = Start-Process $executable -ArgumentList '--self-test', "`"$report`"" -PassThru
        if (-not $process.WaitForExit(120000)) {
            Stop-Process -Id $process.Id
            throw 'Native checks timed out.'
        }
        Get-Content $report
        if ($process.ExitCode -ne 0) { throw 'Native checks failed.' }
    }
    if ($Test -or $SmokeTest) { & (Join-Path $root 'tests\smoke.ps1') -Executable $executable }
    $destination = Join-Path $root "dist\$Architecture"
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Copy-Item $executable (Join-Path $destination 'notepad.exe')
    if ($Architecture -eq 'x64') { Copy-Item $executable (Join-Path $root 'dist\notepad.exe') }
    Write-Output "Portable executable: $(Join-Path $destination 'notepad.exe')"
    if ($Package) {
        $revision = git rev-parse HEAD
        if ($LASTEXITCODE -ne 0) { throw 'Cannot determine source revision for package.' }
        Copy-Item (Join-Path $root 'README.md') (Join-Path $destination 'README.md')
        [ordered]@{ version = $ReleaseVersion; architecture = $Architecture; target = $target; commit = $revision; executable_sha256 = (Get-FileHash $executable -Algorithm SHA256).Hash.ToLowerInvariant() } |
            ConvertTo-Json | Set-Content (Join-Path $destination 'BUILDINFO.json') -Encoding utf8NoBOM
        $packages = Join-Path $root 'dist\packages'
        New-Item -ItemType Directory -Force -Path $packages | Out-Null
        $name = "RustNotepad-$ReleaseVersion-windows-$Architecture.zip"
        $archive = Join-Path $packages $name
        Compress-Archive -Path (Join-Path $destination 'notepad.exe'), (Join-Path $destination 'README.md'), (Join-Path $destination 'BUILDINFO.json') -DestinationPath $archive -Force
        $checksum = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
        "$checksum  $name" | Set-Content "$archive.sha256" -Encoding ascii
        Write-Output "Release package: $archive"
    }
} finally {
    $env:RUSTNOTEPAD_VERSION = $previousVersion
    Pop-Location
}
