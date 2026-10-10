$ErrorActionPreference = "Stop"

$repoDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$candidateRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("dagpipe-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $candidateRoot | Out-Null

try {
    cargo install --path $repoDir --locked --force --root $candidateRoot
    if ($LASTEXITCODE -ne 0) {
        throw "cargo install failed with exit code $LASTEXITCODE"
    }

    $candidate = Join-Path $candidateRoot "bin\dagpipe.exe"
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        $candidate = Join-Path $candidateRoot "bin\dagpipe"
    }

    & $candidate install --source $repoDir
    if ($LASTEXITCODE -ne 0) {
        throw "dagpipe install failed with exit code $LASTEXITCODE"
    }
}
finally {
    Remove-Item -LiteralPath $candidateRoot -Recurse -Force -ErrorAction SilentlyContinue
}
