param(
    [Parameter(Mandatory)][string]$Date,
    [string[]]$ExistingTags = @()
)
$ErrorActionPreference = 'Stop'
if ($Date -cnotmatch '^[0-9]{8}$') { throw 'Release date must use YYYYMMDD.' }
$parsed = [DateTime]::ParseExact($Date, 'yyyyMMdd', [Globalization.CultureInfo]::InvariantCulture)
if ($parsed.Year -lt 2000) { throw 'Release date must be in or after 2000.' }
$base = "0.1.$Date"
$pattern = '^v' + [regex]::Escape($base) + '(?:\.([1-9][0-9]*))?$'
$highest = -1
foreach ($tag in $ExistingTags) {
    if ($tag -cmatch $pattern) {
        $revision = if ($Matches[1]) { [uint16]::Parse($Matches[1], [Globalization.CultureInfo]::InvariantCulture) } else { 0 }
        $highest = [Math]::Max($highest, $revision)
    }
}
if ($highest -eq 65535) { throw "All supported release revisions for $Date are exhausted." }
if ($highest -eq -1) { $base } else { "$base.$($highest + 1)" }
