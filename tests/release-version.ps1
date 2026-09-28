$ErrorActionPreference = 'Stop'
$resolve = Join-Path $PSScriptRoot '..\scripts\release-version.ps1'
function Assert-Version([string]$Date, [string[]]$Tags, [string]$Expected) {
    $actual = & $resolve -Date $Date -ExistingTags $Tags
    if ($actual -cne $Expected) { throw "Expected $Expected; got $actual" }
}
function Assert-Rejected([string]$Date, [string[]]$Tags = @()) {
    $rejected = $false
    try { $null = & $resolve -Date $Date -ExistingTags $Tags } catch { $rejected = $true }
    if (-not $rejected) { throw "Expected version allocation to reject $Date / $Tags" }
}
Assert-Version '20260928' @() '0.1.20260928'
Assert-Version '20260928' @('v0.1.20260928') '0.1.20260928.1'
Assert-Version '20260928' @('v0.1.20260928', 'v0.1.20260928.1') '0.1.20260928.2'
Assert-Version '20260928' @('v0.1.20260928.9', 'v0.1.20260928.10', 'v0.1.20260928.2') '0.1.20260928.11'
Assert-Version '20260928' @('v0.1.20260928.4', 'v0.1.20260928.4') '0.1.20260928.5'
Assert-Version '20260929' @('v0.1.20260928.9', 'v0.1.20260930.2') '0.1.20260929'
Assert-Version '20270101' @('v0.1.20261231.65535') '0.1.20270101'
Assert-Version '20240229' @('other', 'v0.1.20240229-dev', 'v0.1.20240229.1.2') '0.1.20240229'
Assert-Version '20260928' @('v0.1.20260928.65534') '0.1.20260928.65535'
Assert-Rejected '20260928' @('v0.1.20260928.65535')
Assert-Rejected '20260928' @('v0.1.20260928.99999999999999999999999')
foreach ($date in @('19991231', '20260229', '20261301', '20260931', '20260900', '2026-09-28', '２０２６０９２８')) {
    Assert-Rejected $date
}
Write-Output 'PASS: release allocation, numeric ordering, gaps, duplicate reservations, rollover, limits and invalid dates'
