#!/usr/bin/env pwsh
# Native Windows public-CLI acceptance for AppSDK governance reset.
$ErrorActionPreference = 'Stop'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
  $PSNativeCommandUseErrorActionPreference = $false
}

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) { throw "ASSERT FAILED: $Message" }
}

function Invoke-AppSDK {
  param([string[]]$Arguments)
  $output = & $script:appsdk @Arguments 2>&1
  [pscustomobject]@{
    ExitCode = $LASTEXITCODE
    Output = (($output | ForEach-Object { "$_" }) -join "`n")
  }
}

function New-AppSDKProject {
  param([string]$Name)
  $project = Join-Path $script:root $Name
  $result = Invoke-AppSDK @('new', $project)
  Assert-True ($result.ExitCode -eq 0) "appsdk new failed: $($result.Output)"
  $project
}

function Initialize-CleanProject {
  param([string]$Project)
  & git -C $Project init --initial-branch=codex/reset-test | Out-Null
  Assert-True ($LASTEXITCODE -eq 0) 'git init failed'
  & git -C $Project config user.email 'test@appsdk.local'
  Assert-True ($LASTEXITCODE -eq 0) 'git email setup failed'
  & git -C $Project config user.name 'AppSDK Windows Reset Test'
  Assert-True ($LASTEXITCODE -eq 0) 'git user setup failed'
  & git -C $Project add --all
  Assert-True ($LASTEXITCODE -eq 0) 'git add failed'
  & git -C $Project commit -m 'Windows reset fixture' | Out-Null
  Assert-True ($LASTEXITCODE -eq 0) 'git commit failed'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$script:appsdk = Join-Path $repoRoot 'rust/target/release/appsdk.exe'
Assert-True (Test-Path -LiteralPath $script:appsdk -PathType Leaf) "missing AppSDK binary $script:appsdk"

$script:root = Join-Path ([System.IO.Path]::GetTempPath()) ("appsdk windows reset " + [Guid]::NewGuid().ToString('N'))
$appsdkHome = Join-Path $script:root 'appsdk home'
New-Item -ItemType Directory -Path $appsdkHome -Force | Out-Null
$env:APPSDK_HOME = $appsdkHome
$outside = Join-Path $script:root 'external target'
$junction = $null

try {
  Write-Host '== reset success and business data preservation =='
  $project = New-AppSDKProject 'reset project'
  New-Item -ItemType Directory -Path (Join-Path $project 'active') -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $project 'protected/history') -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $project 'generated/old-artifact') -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $project 'business.txt') -Value 'business survives' -NoNewline
  Set-Content -LiteralPath (Join-Path $project 'active/user-data.txt') -Value 'active survives' -NoNewline
  Set-Content -LiteralPath (Join-Path $project 'protected/history/user-data.txt') -Value 'protected survives' -NoNewline
  Set-Content -LiteralPath (Join-Path $project 'generated/old-artifact/output.bin') -Value 'remove generated data' -NoNewline
  Initialize-CleanProject $project

  $reset = Invoke-AppSDK @('reset-governance', $project, '--discard-legacy')
  Assert-True ($reset.ExitCode -eq 0) "reset failed ($($reset.ExitCode)): $($reset.Output)"
  Assert-True ((Get-Content -LiteralPath (Join-Path $project 'business.txt') -Raw) -eq 'business survives') 'business file changed'
  Assert-True ((Get-Content -LiteralPath (Join-Path $project 'active/user-data.txt') -Raw) -eq 'active survives') 'active data changed'
  Assert-True ((Get-Content -LiteralPath (Join-Path $project 'protected/history/user-data.txt') -Raw) -eq 'protected survives') 'protected data changed'
  Assert-True (Test-Path -LiteralPath (Join-Path $project '.appsdk/records/reset-governance-record.json') -PathType Leaf) 'reset receipt missing'
  Assert-True (-not (Test-Path -LiteralPath (Join-Path $project 'generated/old-artifact'))) 'generated artifact survived reset'
  Assert-True ($reset.Output -match '"status"\s*:\s*"completed"') 'reset result did not report completed'

  Write-Host '== junction refusal and external data preservation =='
  $project = New-AppSDKProject 'junction project'
  $projectContractPath = Join-Path $project '.appsdk/project.json'
  $projectContract = Get-Content -LiteralPath $projectContractPath -Raw
  $projectContract = $projectContract.Replace('"generated_root": "generated/**"', '"generated_root": "generated/escape/**"')
  [System.IO.File]::WriteAllText($projectContractPath, $projectContract, [System.Text.UTF8Encoding]::new($false))
  Initialize-CleanProject $project

  New-Item -ItemType Directory -Path $outside -Force | Out-Null
  $outsideMarker = Join-Path $outside 'keep.txt'
  Set-Content -LiteralPath $outsideMarker -Value 'external data survives' -NoNewline
  $junction = Join-Path $project 'generated/escape'
  New-Item -ItemType Directory -Path (Split-Path $junction -Parent) -Force | Out-Null
  New-Item -ItemType Junction -Path $junction -Target $outside -ErrorAction Stop | Out-Null
  $markerHash = (Get-FileHash -LiteralPath $outsideMarker -Algorithm SHA256).Hash

  $reset = Invoke-AppSDK @('reset-governance', $project, '--discard-legacy')
  Assert-True ($reset.ExitCode -ne 0) 'reset followed a junction under the generated root'
  Assert-True ($reset.Output -match 'GOVERNANCE_PATH_SYMLINK|REPARSE') "junction refusal was not explicit: $($reset.Output)"
  Assert-True ((Get-FileHash -LiteralPath $outsideMarker -Algorithm SHA256).Hash -eq $markerHash) 'reset modified external junction target data'
  Assert-True (Test-Path -LiteralPath $projectContractPath -PathType Leaf) 'junction refusal damaged the project contract'
  [System.IO.Directory]::Delete($junction, $false)
  $junction = $null

  Write-Host 'PASS: AppSDK Windows governance reset black-box acceptance'
}
finally {
  if ($junction -and (Test-Path -LiteralPath $junction)) {
    try { [System.IO.Directory]::Delete($junction, $false) } catch { }
  }
  if (Test-Path -LiteralPath $script:root) {
    Remove-Item -LiteralPath $script:root -Recurse -Force -ErrorAction SilentlyContinue
  }
}
