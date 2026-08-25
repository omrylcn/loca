param(
    [Parameter(Mandatory = $true)][string]$Server,
    [Parameter(Mandatory = $true)][string]$Name
)

$ErrorActionPreference = "Stop"
$Server = $Server.TrimEnd("/")
if ($Server -notmatch '^https?://[A-Za-z0-9._-]+(?::[0-9]+)?$') {
    throw "Server must be an http(s) origin without a path"
}
if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$') {
    throw "Name must be 1-64 ASCII letters, digits, dot, dash, or underscore"
}

$secret = Read-Host "Private membership/davet for $Server as $Name" -AsSecureString
$ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secret)
try {
    $token = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
} finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
}
if ([string]::IsNullOrWhiteSpace($token)) { throw "A private membership or davet is required" }

$headers = @{ "x-room-token" = $token }
Invoke-RestMethod -Uri "$Server/health" -TimeoutSec 10 | Out-Null
Invoke-RestMethod -Uri "$Server/rooms" -Headers $headers -TimeoutSec 10 | Out-Null
$who = Invoke-RestMethod -Uri "$Server/whoami" -Headers $headers -TimeoutSec 10
$membership = ""
$davetLoca = [string]$who.loca
if ($who.kind -eq "member") {
    $membership = $token
} elseif ($davetLoca) {
    $claim = Invoke-RestMethod -Method Post -Uri "$Server/membership/claim" `
        -Headers $headers -ContentType "application/json" -Body "{}" -TimeoutSec 10
    $membership = [string]$claim.membership_token
    $who.name = $claim.name
} else {
    throw "Credential is neither a Building membership nor a loca davet"
}
if ([string]$who.name -ne $Name) {
    throw "Credential belongs to '$($who.name)', not requested identity '$Name'"
}
if (-not $membership) { throw "Could not claim Building membership" }

$stem = $Name -replace '[^A-Za-z0-9_-]', '_'
$locaDir = Join-Path $HOME ".loca"
$envPath = Join-Path $locaDir "$stem.env"
New-Item -ItemType Directory -Force $locaDir | Out-Null
$updates = @{
    ROOM_SERVER_URL = $Server
    LOCA_NAME = $Name
    LOCA_MEMBERSHIP = $membership
}
if ($davetLoca -and $who.kind -ne "member") {
    $roomKey = $davetLoca -replace '[^A-Za-z0-9_]', '_'
    $updates["DAVET_$roomKey"] = $token
}
$credentialsScript = Join-Path $PSScriptRoot "credentials.py"
$python = Get-Command python -ErrorAction SilentlyContinue
if (-not $python) { throw "Python 3 is required by the Loca runtime" }
$version = & $python.Source --version 2>&1
if ($LASTEXITCODE -ne 0 -or "$version" -notmatch '^Python 3\.') {
    throw "A working Python 3 runtime is required (the Microsoft Store stub is not sufficient)"
}
$updates | ConvertTo-Json -Compress | & $python.Source $credentialsScript update-json $envPath
if ($LASTEXITCODE -ne 0) { throw "Credential file update failed" }
Write-Host "Loca identity created: $envPath"
Write-Host "The credential remains local and the admin token was never requested."
