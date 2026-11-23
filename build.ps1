# Portable build: produces dist\tuna.exe and dist\tuna-win-x64.zip
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force -Path dist | Out-Null
Copy-Item -Force target\release\tuna.exe dist\tuna.exe
Compress-Archive -Force -Path dist\tuna.exe -DestinationPath dist\tuna-win-x64.zip

Get-Item dist\tuna.exe, dist\tuna-win-x64.zip |
    Select-Object Name, @{n="MB";e={[math]::Round($_.Length/1MB,1)}}, LastWriteTime
