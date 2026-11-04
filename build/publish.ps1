<#
.SYNOPSIS
  Write version.json (the update manifest) beside a built installer.

.EXAMPLE
  pwsh build\publish.ps1 -Installer dist\installer\FPTools-3.0.0-setup.exe `
       -Version 3.0.0 -BaseUrl https://github.com/fpcolin/fp-tools/releases/download/v3.0.0 `
       -Notes "Rewritten in Rust."

  Upload the installer and version.json together. Run this AFTER signing the
  installer: signing rewrites the file, and a checksum taken before it would
  never match what users download.
#>
param(
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [ValidatePattern('^\d+(\.\d+){0,3}$')] [string] $Version,
    [Parameter(Mandatory)] [ValidatePattern('^https://')] [string] $BaseUrl,
    [string] $Notes = ''
)
$ErrorActionPreference = 'Stop'

$file = Get-Item -LiteralPath $Installer
$manifest = [ordered]@{
    version = $Version
    url     = '{0}/{1}' -f $BaseUrl.TrimEnd('/'), $file.Name
    sha256  = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    notes   = $Notes
}
$json = $manifest | ConvertTo-Json
$out = Join-Path $file.DirectoryName 'version.json'
# UTF-8 without a byte-order mark: the Python builds still in the field read
# this file too, and their JSON parser rejects a BOM.
[IO.File]::WriteAllText($out, $json, [Text.UTF8Encoding]::new($false))
Write-Host "Wrote $out"
Write-Host $json
