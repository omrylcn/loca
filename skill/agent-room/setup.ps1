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

$secret = Read-Host "Private membership/invite for $Server as $Name" -AsSecureString
$ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secret)
try {
    $token = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
} finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
}
if ([string]::IsNullOrWhiteSpace($token)) { throw "A private membership or invite is required" }

$headers = @{ "x-room-token" = $token }
Invoke-RestMethod -Uri "$Server/health" -TimeoutSec 10 | Out-Null
Invoke-RestMethod -Uri "$Server/rooms" -Headers $headers -TimeoutSec 10 | Out-Null
$who = Invoke-RestMethod -Uri "$Server/whoami" -Headers $headers -TimeoutSec 10
$inviteLoca = [string]$who.loca
if ($who.kind -eq "member") {
    if ([string]$who.name -ne $Name) {
        throw "Credential belongs to '$($who.name)', not requested identity '$Name'"
    }
} elseif (-not $inviteLoca) {
    throw "Credential is neither a Building membership nor a loca invite"
}

$credentialsScript = Join-Path $PSScriptRoot "credentials.py"
$python = Get-Command python -ErrorAction SilentlyContinue
if (-not $python) { throw "Python 3 is required by the Loca runtime" }
$version = & $python.Source --version 2>&1
if ($LASTEXITCODE -ne 0 -or "$version" -notmatch '^Python 3\.') {
    throw "A working Python 3 runtime is required (the Microsoft Store stub is not sufficient)"
}

$stem = $Name -replace '[^A-Za-z0-9_-]', '_'
$locaDir = Join-Path $HOME ".loca"
$envPath = Join-Path $locaDir "$stem.env"
New-Item -ItemType Directory -Force $locaDir | Out-Null
$updates = @{
    ROOM_SERVER_URL = $Server
    LOCA_NAME = $Name
}
if ($who.kind -eq "member") {
    $updates["LOCA_MEMBERSHIP"] = $token
} else {
    # An invite proves which member holds it: claim this set-up's own Lobby
    # key with it, exactly as connect.sh does. The helper keeps the set-up's
    # claim id in a private file (a re-run resumes the same set-up), files the
    # verified key in the identity env and acknowledges it. The invite travels
    # on stdin, never on a command line.
    $claimScript = Join-Path $PSScriptRoot "join_request.py"
    $claimState = Join-Path $locaDir "$stem.claim"
    $token | & $python.Source $claimScript claim $Server $Name $claimState $envPath
    switch ($LASTEXITCODE) {
        0 { }
        6 { Write-Host "Run setup again once to finish the claim (the key is already filed)." }
        4 { throw "Credential belongs to another identity, not requested identity '$Name'" }
        default { throw "No Building membership was claimed (see the message above)" }
    }
    $roomKey = $inviteLoca -replace '[^A-Za-z0-9_]', '_'
    $updates["DAVET_$roomKey"] = $token
}
$updates | ConvertTo-Json -Compress | & $python.Source $credentialsScript update-json $envPath
if ($LASTEXITCODE -ne 0) { throw "Credential file update failed" }
Write-Host "Loca identity created: $envPath"
Write-Host "The credential remains local and the admin token was never requested."
