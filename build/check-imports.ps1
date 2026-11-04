<#
.SYNOPSIS
  Fail if an executable imports a DLL that is not on the allowlist.

.DESCRIPTION
  Reads the PE import and delay-import tables directly, so no SDK tools are
  needed. Used by the release workflow; also handy locally:

    pwsh build\check-imports.ps1 target\release\FPVendorCafeConverter.exe
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [string] $Allowlist = (Join-Path $PSScriptRoot 'allowed-dlls.txt')
)
$ErrorActionPreference = 'Stop'

$bytes = [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Exe))
function U16([int] $o) { [BitConverter]::ToUInt16($bytes, $o) }
function U32([int] $o) { [BitConverter]::ToUInt32($bytes, $o) }

$pe = U32 0x3C
if ((U32 $pe) -ne 0x00004550) { throw "$Exe is not a PE file" }
$sectionCount = U16 ($pe + 6)
$optSize = U16 ($pe + 20)
$opt = $pe + 24
$dirs = switch (U16 $opt) { 0x20B { $opt + 112 } 0x10B { $opt + 96 } default { throw 'unknown optional header' } }
$sections = $opt + $optSize

function Offset([uint32] $rva) {
    for ($i = 0; $i -lt $sectionCount; $i++) {
        $s = $sections + 40 * $i
        $va = U32 ($s + 12)
        $size = [Math]::Max((U32 ($s + 8)), (U32 ($s + 16)))
        if ($rva -ge $va -and $rva -lt $va + $size) { return [int]($rva - $va + (U32 ($s + 20))) }
    }
    throw ('RVA 0x{0:x} is outside every section' -f $rva)
}
function CString([uint32] $rva) {
    $o = Offset $rva
    $end = [Array]::IndexOf($bytes, [byte]0, $o)
    [Text.Encoding]::ASCII.GetString($bytes, $o, $end - $o)
}

$dlls = [Collections.Generic.List[string]]::new()
# Data directory 1 = imports (20-byte descriptors), 13 = delay imports (32-byte).
foreach ($entry in @(@{ Index = 1; Size = 20; NameAt = 12 }, @{ Index = 13; Size = 32; NameAt = 4 })) {
    $rva = U32 ($dirs + 8 * $entry.Index)
    if ($rva -eq 0) { continue }
    $o = Offset $rva
    while ($true) {
        $nameRva = U32 ($o + $entry.NameAt)
        if ($nameRva -eq 0) { break }
        $dlls.Add((CString $nameRva))
        $o += $entry.Size
    }
}

$rules = Get-Content -LiteralPath $Allowlist |
    ForEach-Object { $_.Trim().ToLowerInvariant() } |
    Where-Object { $_ -and -not $_.StartsWith('#') }
function Allowed([string] $dll) {
    return $rules -contains $dll.ToLowerInvariant()
}

$bad = @()
foreach ($dll in ($dlls | Sort-Object -Unique)) {
    $ok = Allowed $dll
    Write-Host ('{0,-40} {1}' -f $dll, $(if ($ok) { 'allowed' } else { 'NOT ALLOWED' }))
    if (-not $ok) { $bad += $dll }
}
if ($bad) {
    Write-Error ("{0} imports DLLs that are not in {1}: {2}. Remove the dependency that brought them in, or add them to the list deliberately and tell IT." -f $Exe, (Split-Path $Allowlist -Leaf), ($bad -join ', '))
    exit 1
}
Write-Host 'Import check passed.'
