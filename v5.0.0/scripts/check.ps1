# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
function Invoke-Cargo {
    param([Parameter(Mandatory)][string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}
Push-Location (Join-Path $PSScriptRoot '..')
try {
    Invoke-Cargo -CargoArguments @('fmt', '--all', '--check')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--all-targets')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--doc')
    Invoke-Cargo -CargoArguments @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo -CargoArguments @('build', '--locked', '--release')
    # The offline demo, into a fresh folder that git ignores.
    if (Test-Path -LiteralPath 'target\demo') {
        Remove-Item -LiteralPath 'target\demo' -Recurse -Force
    }
    Invoke-Cargo -CargoArguments @('run', '--locked', '--release', '--', '--db', 'target\demo\demo-case.db', 'demo', '--out', 'target\demo\demo.html')
    Write-Host 'OSINTropy validation passed.'
}
finally {
    Pop-Location
}
