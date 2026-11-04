<#
.SYNOPSIS
  Create a self-signed code-signing certificate for signing Flooring Partners
  Tools in-house, instead of buying one.

.DESCRIPTION
  Run once, on Windows, in PowerShell:

    powershell -ExecutionPolicy Bypass -File build\new-signing-cert.ps1

  It writes two files and copies a third value to the clipboard:

    FP-CodeSigning.pfx  The certificate WITH its private key. Whoever has this
                        file and its password can sign programs that your
                        ThreatLocker policy will trust, so keep it secret: put
                        it in GitHub (below), store a backup in your password
                        manager, and delete loose copies.
    FP-CodeSigning.cer  The public certificate only. Safe to share. Give it to
                        IT: it is what the PCs and ThreatLocker trust.
    clipboard           The .pfx as base64, ready to paste into the GitHub
                        secret SIGNING_PFX_BASE64.

  Then, in the GitHub repository: Settings -> Secrets and variables ->
  Actions -> New repository secret:
    SIGNING_PFX_BASE64    paste the clipboard
    SIGNING_PFX_PASSWORD  the password you chose here
  The next tagged release is signed automatically.

  The certificate is valid for -Years years (default 10). Signatures are
  timestamped, so programs signed before it expires stay valid afterwards;
  you only need a new certificate to sign new releases after that date.
#>
param(
    [string] $Subject = 'CN=Flooring Partners, O=Flooring Partners',
    [int] $Years = 10,
    [string] $OutDir = (Get-Location).Path
)
$ErrorActionPreference = 'Stop'

$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject `
    -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 `
    -KeyExportPolicy Exportable -CertStoreLocation Cert:\CurrentUser\My `
    -NotAfter (Get-Date).AddYears($Years)

$password = Read-Host -AsSecureString 'Choose a password for the .pfx file'
$pfx = Join-Path $OutDir 'FP-CodeSigning.pfx'
$cer = Join-Path $OutDir 'FP-CodeSigning.cer'
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $password | Out-Null
Export-Certificate -Cert $cert -FilePath $cer | Out-Null

# The exported .pfx is now the only copy of the private key; don't leave a
# second one in this user's certificate store.
Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($cert.Thumbprint)"

[Convert]::ToBase64String([IO.File]::ReadAllBytes($pfx)) | Set-Clipboard

Write-Host ''
Write-Host "Created:  $pfx   (secret)"
Write-Host "          $cer   (public - give this to IT)"
Write-Host "Subject:  $($cert.Subject)"
Write-Host "Thumbprint: $($cert.Thumbprint)"
Write-Host "Valid until: $($cert.NotAfter.ToString('yyyy-MM-dd'))"
Write-Host ''
Write-Host 'The .pfx (base64) is on the clipboard: paste it into the GitHub secret SIGNING_PFX_BASE64,'
Write-Host 'and put the password in SIGNING_PFX_PASSWORD.'
