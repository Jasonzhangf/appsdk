#!/usr/bin/env pwsh
# Windows black-box acceptance for the DAGPipe CLI/SDK installer.
# Runs on a native Windows/MSVC runner and exercises the real install entry:
# install + upgrade, installed CLI identity, a real Cargo consumer built from the
# reported SDK path, a real junction/reparse refusal, and a real in-use
# executable replacement conflict. It never simulates success and never skips.
$ErrorActionPreference = 'Stop'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
  $PSNativeCommandUseErrorActionPreference = $false
}

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) { throw "ASSERT FAILED: $Message" }
}

function Get-Sha256 {
  param([string]$Path)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $stream = [System.IO.File]::OpenRead($Path)
    try { return -join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) }
    finally { $stream.Dispose() }
  } finally { $sha.Dispose() }
}

function Get-InstalledFilesManifest {
  param([string]$SdkPath)
  $rootFull = (Resolve-Path -LiteralPath $SdkPath).Path
  $lines = New-Object System.Collections.Generic.List[string]
  foreach ($file in (Get-ChildItem -LiteralPath $SdkPath -Recurse -File -Force)) {
    $relative = $file.FullName.Substring($rootFull.Length).TrimStart('\', '/') -replace '\\', '/'
    if ($relative -eq '.installed-files' -or $relative -eq '.installed-tree') { continue }
    $lines.Add((Get-Sha256 $file.FullName) + '  ./' + $relative)
  }
  $sorted = $lines.ToArray()
  [Array]::Sort($sorted, [System.StringComparer]::Ordinal)
  return ($sorted -join "`n")
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$installPs1 = Join-Path $repoRoot 'dagpipe/scripts/install.ps1'
$graphFile = Join-Path $repoRoot 'dagpipe/examples/governance_graph.json'
$skillSource = Join-Path $repoRoot 'dagpipe/.agents/skills/dagpipe-runtime/SKILL.md'

Assert-True (Test-Path -LiteralPath $installPs1 -PathType Leaf) "missing installer $installPs1"
Assert-True (Test-Path -LiteralPath $graphFile -PathType Leaf) "missing graph $graphFile"
Assert-True (Test-Path -LiteralPath $skillSource -PathType Leaf) "missing skill source $skillSource"

# Isolated, space-containing roots. The real user root is never modified; the
# real Cargo/Rustup caches are preserved so offline resolution keeps working.
$realUserProfile = $env:USERPROFILE
$realCargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } elseif ($realUserProfile) { Join-Path $realUserProfile '.cargo' } else { $null }
$realRustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } elseif ($realUserProfile) { Join-Path $realUserProfile '.rustup' } else { $null }

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("dagpipe windows harness " + [Guid]::NewGuid().ToString('N'))
$home = Join-Path $root 'user home'
$localAppData = Join-Path $root 'local app data'
$cargoRoot = Join-Path $root 'cargo install root'
$consumerRoot = Join-Path $root 'consumer project'
$externalTarget = Join-Path $root 'external sdk target'
New-Item -ItemType Directory -Path $root | Out-Null

$sdkPath = Join-Path $localAppData 'dagpipe\sdk'
$skillPath = Join-Path $home '.agents\skills\dagpipe-runtime\SKILL.md'
$installedExe = Join-Path $cargoRoot 'bin\dagpipe.exe'

$env:HOME = $home
$env:USERPROFILE = $home
$env:LOCALAPPDATA = $localAppData
$env:CARGO_INSTALL_ROOT = $cargoRoot
if ($realCargoHome) { $env:CARGO_HOME = $realCargoHome }
if ($realRustupHome) { $env:RUSTUP_HOME = $realRustupHome }

$script:installSeq = 0
$junctionCreated = $false

function Invoke-Installer {
  $script:installSeq++
  $log = Join-Path $root ("installer-{0}.log" -f $script:installSeq)
  & pwsh -NoProfile -File $installPs1 *> $log
  return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Log = $log }
}

function Get-InstallerLog {
  param([pscustomobject]$Result)
  if (Test-Path -LiteralPath $Result.Log) { return (Get-Content -LiteralPath $Result.Log -Raw) }
  return ''
}

function Invoke-Dagpipe {
  param([string[]]$Arguments)
  $out = & $installedExe @Arguments
  return [pscustomobject]@{
    ExitCode = $LASTEXITCODE
    Output = (($out | ForEach-Object { "$_" }) -join "`n")
  }
}

Write-Host "DAGPipe Windows harness root: $root"

try {
  Write-Host '== initial install =='
  $result = Invoke-Installer
  Assert-True ($result.ExitCode -eq 0) "initial install failed ($($result.ExitCode)):`n$(Get-InstallerLog $result)"
  Assert-True (Test-Path -LiteralPath $installedExe -PathType Leaf) 'installed CLI missing'
  Assert-True (Test-Path -LiteralPath $sdkPath -PathType Container) 'installed SDK missing'
  Assert-True (Test-Path -LiteralPath (Join-Path $sdkPath 'Cargo.toml') -PathType Leaf) 'installed SDK manifest missing'
  Assert-True (Test-Path -LiteralPath (Join-Path $sdkPath 'src') -PathType Container) 'installed SDK src missing'
  Assert-True (Test-Path -LiteralPath $skillPath -PathType Leaf) 'installed Skill missing'
  Assert-True (Test-Path -LiteralPath (Join-Path $sdkPath '.installed-files') -PathType Leaf) 'SDK file manifest missing'
  Assert-True (Test-Path -LiteralPath (Join-Path $sdkPath '.installed-tree') -PathType Leaf) 'SDK tree manifest missing'

  Write-Host '== installed CLI identity, sdk path, graph validate =='
  $versionResult = Invoke-Dagpipe @('--version')
  Assert-True ($versionResult.ExitCode -eq 0) "dagpipe --version failed ($($versionResult.ExitCode))"
  $sdkManifestText = Get-Content -LiteralPath (Join-Path $sdkPath 'Cargo.toml') -Raw
  $expectedVersion = [regex]::Match($sdkManifestText, '(?m)^\s*version\s*=\s*"([^"]+)"').Groups[1].Value
  Assert-True (-not [string]::IsNullOrWhiteSpace($expectedVersion)) 'could not read the installed SDK version'
  Assert-True ($versionResult.Output.Trim() -eq "dagpipe $expectedVersion") "version mismatch: '$($versionResult.Output.Trim())' != 'dagpipe $expectedVersion'"

  $sdkResult = Invoke-Dagpipe @('sdk', 'path')
  Assert-True ($sdkResult.ExitCode -eq 0) "dagpipe sdk path failed ($($sdkResult.ExitCode))"
  $sdkOutput = $sdkResult.Output.Trim()
  $expectedSdkPath = $sdkPath.Replace('\', '/')
  Assert-True ($sdkOutput -eq $expectedSdkPath) "sdk path mismatch: '$sdkOutput' != '$expectedSdkPath'"
  Assert-True (Test-Path -LiteralPath $sdkOutput -PathType Container) "reported SDK path does not exist: $sdkOutput"

  $graphResult = Invoke-Dagpipe @('graph', 'validate', $graphFile)
  Assert-True ($graphResult.ExitCode -eq 0) "graph validate failed ($($graphResult.ExitCode)): $($graphResult.Output)"
  Assert-True ($graphResult.Output -match 'valid DAG') "graph validate did not report a valid DAG: $($graphResult.Output)"

  Write-Host '== installed bytes =='
  Assert-True ((Get-Sha256 $skillPath) -eq (Get-Sha256 $skillSource)) 'installed Skill bytes differ from source'
  $expectedFiles = (Get-Content -LiteralPath (Join-Path $sdkPath '.installed-files') -Raw) -replace "`r`n", "`n"
  $actualFiles = (Get-InstalledFilesManifest -SdkPath $sdkPath) -replace "`r`n", "`n"
  Assert-True ($actualFiles.TrimEnd() -eq $expectedFiles.TrimEnd()) 'installed SDK file manifest does not match recomputed hashes'

  Write-Host '== real Cargo consumer from the reported SDK path =='
  New-Item -ItemType Directory -Path (Join-Path $consumerRoot 'src') -Force | Out-Null
  $consumerManifest = @"
[package]
name = "dagpipe-consumer"
version = "0.1.0"
edition = "2021"

[dependencies]
pipeline_runtime = { path = "$sdkOutput" }
"@
  Set-Content -LiteralPath (Join-Path $consumerRoot 'Cargo.toml') -Value $consumerManifest
  Set-Content -LiteralPath (Join-Path $consumerRoot 'src\main.rs') -Value 'fn main() { let _ = pipeline_runtime::Registry::default(); }'
  & cargo check --manifest-path (Join-Path $consumerRoot 'Cargo.toml') --offline
  Assert-True ($LASTEXITCODE -eq 0) "real Cargo consumer check failed ($LASTEXITCODE)"

  Write-Host '== upgrade =='
  $result = Invoke-Installer
  Assert-True ($result.ExitCode -eq 0) "upgrade failed ($($result.ExitCode)):`n$(Get-InstallerLog $result)"
  Assert-True (Test-Path -LiteralPath $installedExe -PathType Leaf) 'CLI missing after upgrade'
  Assert-True (Test-Path -LiteralPath (Join-Path $sdkPath 'Cargo.toml') -PathType Leaf) 'SDK missing after upgrade'

  Write-Host '== in-use executable replacement conflict =='
  $beforeHash = Get-Sha256 $installedExe
  $handle = [System.IO.File]::Open($installedExe, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
  try {
    $result = Invoke-Installer
    Assert-True ($result.ExitCode -ne 0) 'installer replaced an executable held open by another handle'
    $text = Get-InstallerLog $result
    Assert-True ($text -match 'preserve existing DAGpipe CLI') "in-use failure did not report the preserved CLI:`n$text"
    Assert-True ((Get-Sha256 $installedExe) -eq $beforeHash) 'in-use failure changed the installed CLI'
    $versionAfter = Invoke-Dagpipe @('--version')
    Assert-True ($versionAfter.ExitCode -eq 0) 'previous installation no longer runs after a refused replacement'
  } finally {
    $handle.Dispose()
  }

  Write-Host '== upgrade after releasing the handle =='
  $result = Invoke-Installer
  Assert-True ($result.ExitCode -eq 0) "upgrade after releasing the handle failed ($($result.ExitCode)):`n$(Get-InstallerLog $result)"
  Assert-True (Test-Path -LiteralPath $installedExe -PathType Leaf) 'CLI missing after released-handle upgrade'

  Write-Host '== junction / reparse refusal =='
  Remove-Item -LiteralPath $sdkPath -Recurse -Force
  New-Item -ItemType Directory -Path $externalTarget | Out-Null
  $marker = Join-Path $externalTarget 'external-keep.txt'
  Set-Content -LiteralPath $marker -Value 'external target content must survive'
  New-Item -ItemType Directory -Path (Join-Path $externalTarget 'src') | Out-Null
  Set-Content -LiteralPath (Join-Path $externalTarget 'Cargo.toml') -Value "[package]`nname = `"pipeline_runtime`"`nversion = `"0.0.0`"`nrepository = `"https://github.com/Jasonzhangf/appsdk`"`n"
  $markerHash = Get-Sha256 $marker
  try {
    New-Item -ItemType Junction -Path $sdkPath -Target $externalTarget -ErrorAction Stop | Out-Null
    $junctionCreated = $true
  } catch {
    throw "cannot create a directory junction for the reparse refusal test: $($_.Exception.Message)"
  }
  $attributes = [System.IO.File]::GetAttributes($sdkPath)
  Assert-True (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq [System.IO.FileAttributes]::ReparsePoint) 'created SDK path is not a reparse point'
  $result = Invoke-Installer
  Assert-True ($result.ExitCode -ne 0) 'installer followed a reparse-point SDK path'
  $text = Get-InstallerLog $result
  Assert-True ($text -match 'reparse|symlink') "reparse refusal did not mention reparse/symlink:`n$text"
  Assert-True (Test-Path -LiteralPath $marker -PathType Leaf) 'installer removed external target content'
  Assert-True ((Get-Sha256 $marker) -eq $markerHash) 'installer modified external target content'
  [System.IO.Directory]::Delete($sdkPath, $false)
  $junctionCreated = $false
  Assert-True (Test-Path -LiteralPath $marker -PathType Leaf) 'removing the junction removed external target content'

  Write-Host 'PASS: DAGPipe Windows install harness'
}
finally {
  if ($junctionCreated -and (Test-Path -LiteralPath $sdkPath)) {
    try { [System.IO.Directory]::Delete($sdkPath, $false) } catch { }
  }
  if (Test-Path -LiteralPath $root) {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
  }
}
