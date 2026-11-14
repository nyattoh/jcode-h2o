<#
.SYNOPSIS
Windows PowerShell helper for building, testing, and running jcode-h2o (experiments/h2o-drift).

.DESCRIPTION
Works around the POSIX rustc-wrapper in .cargo/config.toml (os error 193 on Windows) by setting
CARGO_BUILD_RUSTC_WRAPPER="" on the child cargo ProcessStartInfo environment (works in both
Windows PowerShell 5.1 and PowerShell 7+). Does not modify .cargo/config.toml, Linux/macOS
scripts, or jcode core, and never stores secrets or automatically calls paid APIs.

.EXAMPLE
.\scripts\h2o.ps1 test
.\scripts\h2o.ps1 clippy
.\scripts\h2o.ps1 build-release
.\scripts\h2o.ps1 run-native
.\scripts\h2o.ps1 run-ui -Port 8787
.\scripts\h2o.ps1 serve-daemon -Socket $env:TEMP\jcode.sock
.\scripts\h2o.ps1 run-monitor --help
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$H2oRoot = Join-Path $RepoRoot 'experiments\h2o-drift'
$ManifestPath = Join-Path $H2oRoot 'Cargo.toml'
$TargetDir = Join-Path $RepoRoot 'target\h2o-native'
$ReleaseDir = Join-Path $TargetDir 'release'
$DefaultUiAssets = Join-Path $H2oRoot 'ui'

function Show-H2oHelp {
    @'
Usage: .\scripts\h2o.ps1 <command> [options]

Commands:
  help            Show this message
  test            Run offline h2o-drift Rust tests (and Web UI JS tests if node is available)
                    Options: -Features <default|all|sdk|jev|ui|native-ui|embedded-runtime|"native-ui jev"> [-Online] [-- <test-args>]
  clippy          Run cargo fmt --check and cargo clippy --all-features --all-targets -- -D warnings
                    Options: [-Online] [-SkipFmt]
  build-release   Build release binaries (h2o-native, h2o-monitor, h2o-ui) into target/h2o-native/release/
                    Options: [-Online]
  run-native      Run h2o-native (single-binary desktop app with embedded runtime; switch to Demo in UI)
                    Options: [-Snapshot <file>] [-Cargo] [-Online] [--help]
  run-ui          Run h2o-ui static loopback server on 127.0.0.1
                    Options: [-Port <0-65535>] [-Assets <dir>] [-Snapshot <file>] [-Cargo] [-Online] [--help]
  serve-daemon    Start `jcode serve` foreground daemon (never stores secrets or calls APIs on its own)
                    Options: [-Socket <path>] [-Provider <name>] [-Model <name>] [--help]
  run-monitor     Run h2o-monitor against an existing harness API socket (requires -EnableJev / --enable-jev)
                    Options: [-EnableJev] [-Socket <path>] [-Spec <json>] [-Journal <dir>] [-Sources <dir>]
                             [-Owner <id>] [-Worker <id>] [-JevBaseUrl <url>] [-MinProbability <p>]
                             [-DeadlineMs <ms>] [-OutputCapBytes <n>] [-IncludeContextBytes] [-PollMs <ms>]
                             [-Cargo] [-Online] [--help]

Note:
  run-native, run-ui, and run-monitor prefer prebuilt release binaries in target/h2o-native/release/;
  re-run `.\scripts\h2o.ps1 build-release` after source changes (or pass `-Cargo`).

Examples:
  .\scripts\h2o.ps1 test
  .\scripts\h2o.ps1 clippy
  .\scripts\h2o.ps1 build-release
  .\scripts\h2o.ps1 run-ui -Port 8787
  .\scripts\h2o.ps1 run-native
  .\scripts\h2o.ps1 run-monitor -EnableJev -Socket <api-sock> -Spec experiments\h2o-drift\examples\sample-spec.json -Journal <dir> -Sources <dir> -Owner <id> -Worker <id>
'@ | Write-Host
}

function Format-ProcessArgument {
    param([string]$Value)

    if ($Value.Length -eq 0) {
        return '""'
    }
    if ($Value -notmatch '[\s"]') {
        return $Value
    }
    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

function Invoke-Cargo {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$CargoArgs
    )

    $cargoExe = (Get-Command 'cargo' -ErrorAction Stop).Source
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $cargoExe
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.Arguments = (@($CargoArgs) | ForEach-Object { Format-ProcessArgument $_ }) -join ' '

    # PowerShell 5.1 deletes env vars assigned "", so set CARGO_BUILD_RUSTC_WRAPPER="" directly on ProcessStartInfo.
    # If the caller explicitly set RUSTC_WRAPPER (e.g. sccache), Cargo gives RUSTC_WRAPPER precedence.
    $psi.EnvironmentVariables['CARGO_BUILD_RUSTC_WRAPPER'] = ''

    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        $proc.WaitForExit()
        if ($proc.ExitCode -ne 0) {
            exit $proc.ExitCode
        }
    } finally {
        $proc.Dispose()
    }
}

function Resolve-PrebuiltBinary {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [switch]$ForceCargo
    )

    if ($ForceCargo) {
        return $null
    }
    $exeName = if ($env:OS -eq 'Windows_NT') { "$Name.exe" } else { $Name }
    $candidate = Join-Path $ReleaseDir $exeName
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
        return $candidate
    }
    return $null
}

function ConvertTo-ArgArray {
    param([object]$Value)

    if ($null -eq $Value) {
        return ,([string[]]@())
    }
    return ,([string[]]@($Value))
}

function Invoke-PrebuiltOrCargo {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BinaryName,
        [Parameter(Mandatory = $true)]
        [string]$Features,
        [AllowEmptyCollection()]
        [string[]]$BinaryArgs = @(),
        [switch]$ForceCargo,
        [switch]$Online
    )

    $argList = ConvertTo-ArgArray $BinaryArgs
    $prebuilt = Resolve-PrebuiltBinary -Name $BinaryName -ForceCargo:$ForceCargo
    if ($prebuilt) {
        & $prebuilt @argList
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
        return
    }

    $cargoArgs = [System.Collections.Generic.List[string]]::new()
    $cargoArgs.Add('run')
    if (-not $Online) {
        $cargoArgs.Add('--offline')
    }
    $cargoArgs.Add('--release')
    $cargoArgs.Add('--manifest-path')
    $cargoArgs.Add($ManifestPath)
    $cargoArgs.Add('--target-dir')
    $cargoArgs.Add($TargetDir)
    $cargoArgs.Add('--features')
    $cargoArgs.Add($Features)
    $cargoArgs.Add('--bin')
    $cargoArgs.Add($BinaryName)
    if ($argList.Count -gt 0) {
        $cargoArgs.Add('--')
        foreach ($arg in $argList) {
            $cargoArgs.Add($arg)
        }
    }
    Invoke-Cargo -CargoArgs $cargoArgs.ToArray()
}

function Read-OptionValue {
    param(
        [AllowEmptyCollection()]
        [string[]]$Items = @(),
        [ref]$Index,
        [string]$OptionName
    )

    $arr = @($Items)
    if ($Index.Value + 1 -ge $arr.Count) {
        throw "$OptionName requires a value."
    }
    $Index.Value++
    return $arr[$Index.Value]
}

function Find-JcodeExecutable {
    $cmd = Get-Command 'jcode' -ErrorAction SilentlyContinue
    if ($cmd) {
        return $cmd.Source
    }
    if ($env:LOCALAPPDATA) {
        $candidate = Join-Path $env:LOCALAPPDATA 'jcode\bin\jcode.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw 'jcode executable not found on PATH or in %LOCALAPPDATA%\jcode\bin\jcode.exe.'
}

function Invoke-H2oTest {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $restArgs = ConvertTo-ArgArray $Rest
    $features = $null
    $online = $false
    $passThrough = [System.Collections.Generic.List[string]]::new()

    for ($i = 0; $i -lt $restArgs.Count; $i++) {
        $arg = $restArgs[$i]
        switch -Regex ($arg) {
            '^(-Features|--features)$' {
                $features = Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg
            }
            '^(-Online|--online)$' {
                $online = $true
            }
            '^(-h|--help|-Help)$' {
                Show-H2oHelp
                return
            }
            default {
                $passThrough.Add($arg)
            }
        }
    }

    if ($null -ne $features) {
        $cargoArgs = [System.Collections.Generic.List[string]]::new()
        $cargoArgs.Add('test')
        if (-not $online) {
            $cargoArgs.Add('--offline')
        }
        $cargoArgs.Add('--locked')
        $cargoArgs.Add('--manifest-path')
        $cargoArgs.Add($ManifestPath)
        $cargoArgs.Add('--target-dir')
        $cargoArgs.Add($TargetDir)
        if ($features -eq 'all') {
            $cargoArgs.Add('--all-features')
        } elseif ($features -eq 'embedded-runtime') {
            $cargoArgs.Add('--features')
            $cargoArgs.Add('native-ui,embedded-runtime')
        } elseif ($features -ne '' -and $features -ne 'default') {
            $cargoArgs.Add('--features')
            $cargoArgs.Add($features)
        }
        foreach ($extra in $passThrough) {
            $cargoArgs.Add($extra)
        }
        Invoke-Cargo -CargoArgs $cargoArgs.ToArray()
        return
    }

    $baseArgs = @('test')
    if (-not $online) {
        $baseArgs += '--offline'
    }
    $baseArgs += @('--locked', '--manifest-path', $ManifestPath, '--target-dir', $TargetDir)

    Write-Host '==> Running h2o-drift offline tests (features: native-ui jev)' -ForegroundColor Cyan
    Invoke-Cargo -CargoArgs ($baseArgs + @('--features', 'native-ui jev') + $passThrough.ToArray())

    Write-Host '==> Running h2o-drift embedded runtime tests (features: native-ui,embedded-runtime)' -ForegroundColor Cyan
    Invoke-Cargo -CargoArgs ($baseArgs + @('--features', 'native-ui,embedded-runtime', '--test', 'embedded_runtime') + $passThrough.ToArray())

    Write-Host '==> Running h2o-drift offline tests (default features)' -ForegroundColor Cyan
    Invoke-Cargo -CargoArgs ($baseArgs + $passThrough.ToArray())

    $uiTestDir = Join-Path $DefaultUiAssets 'tests'
    $node = Get-Command 'node' -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $uiTestDir -PathType Container) -and $node) {
        $jsTests = @(Get-ChildItem -LiteralPath $uiTestDir -Filter '*.test.mjs' -File | ForEach-Object { $_.FullName })
        if ($jsTests.Count -gt 0) {
            Write-Host '==> Running Web UI JS tests (node --test)' -ForegroundColor Cyan
            & $node.Source --test @jsTests
            if ($LASTEXITCODE -ne 0) {
                exit $LASTEXITCODE
            }
        }
    } elseif (Test-Path -LiteralPath $uiTestDir -PathType Container) {
        Write-Host '[note] node not found; skipping Web UI JS tests.' -ForegroundColor Yellow
    }
}

function Invoke-H2oClippy {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $online = $false
    $skipFmt = $false
    foreach ($arg in (ConvertTo-ArgArray $Rest)) {
        switch -Regex ($arg) {
            '^(-Online|--online)$' { $online = $true }
            '^(-SkipFmt|--skip-fmt)$' { $skipFmt = $true }
            '^(-h|--help|-Help)$' { Show-H2oHelp; return }
            default { throw "Unknown clippy option: $arg" }
        }
    }

    if (-not $skipFmt) {
        Write-Host '==> Checking formatting (cargo fmt --check)' -ForegroundColor Cyan
        Invoke-Cargo -CargoArgs @('fmt', '--manifest-path', $ManifestPath, '--', '--check')
    }

    Write-Host '==> Running clippy (--all-features --all-targets -D warnings)' -ForegroundColor Cyan
    $clippyArgs = [System.Collections.Generic.List[string]]::new()
    $clippyArgs.Add('clippy')
    if (-not $online) {
        $clippyArgs.Add('--offline')
    }
    $clippyArgs.Add('--locked')
    $clippyArgs.Add('--manifest-path')
    $clippyArgs.Add($ManifestPath)
    $clippyArgs.Add('--target-dir')
    $clippyArgs.Add($TargetDir)
    $clippyArgs.Add('--all-features')
    $clippyArgs.Add('--all-targets')
    $clippyArgs.Add('--')
    $clippyArgs.Add('-D')
    $clippyArgs.Add('warnings')
    Invoke-Cargo -CargoArgs $clippyArgs.ToArray()
}

function Invoke-H2oBuildRelease {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $online = $false
    $passThrough = [System.Collections.Generic.List[string]]::new()
    foreach ($arg in (ConvertTo-ArgArray $Rest)) {
        switch -Regex ($arg) {
            '^(-Online|--online)$' { $online = $true }
            '^(-h|--help|-Help)$' { Show-H2oHelp; return }
            default { $passThrough.Add($arg) }
        }
    }

    $cargoArgs = [System.Collections.Generic.List[string]]::new()
    $cargoArgs.Add('build')
    if (-not $online) {
        $cargoArgs.Add('--offline')
    }
    $cargoArgs.Add('--locked')
    $cargoArgs.Add('--release')
    $cargoArgs.Add('--manifest-path')
    $cargoArgs.Add($ManifestPath)
    $cargoArgs.Add('--target-dir')
    $cargoArgs.Add($TargetDir)
    $cargoArgs.Add('--all-features')
    $cargoArgs.Add('--bins')
    foreach ($extra in $passThrough) {
        $cargoArgs.Add($extra)
    }

    Write-Host '==> Building release binaries (h2o-native, h2o-monitor, h2o-ui)' -ForegroundColor Cyan
    Invoke-Cargo -CargoArgs $cargoArgs.ToArray()

    Write-Host "Release binaries ready in ${ReleaseDir}:" -ForegroundColor Green
    foreach ($name in @('h2o-native', 'h2o-monitor', 'h2o-ui')) {
        $exe = if ($env:OS -eq 'Windows_NT') { "$name.exe" } else { $name }
        Write-Host "  $(Join-Path $ReleaseDir $exe)"
    }
}

function Invoke-H2oRunNative {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $restArgs = ConvertTo-ArgArray $Rest
    $forceCargo = $false
    $online = $false
    $binArgs = [System.Collections.Generic.List[string]]::new()

    for ($i = 0; $i -lt $restArgs.Count; $i++) {
        $arg = $restArgs[$i]
        switch -Regex ($arg) {
            '^(-Cargo|--cargo)$' { $forceCargo = $true }
            '^(-Online|--online)$' { $online = $true }
            '^(-Snapshot|--snapshot)$' {
                $binArgs.Add('--snapshot')
                $binArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            default {
                $binArgs.Add($arg)
            }
        }
    }

    Invoke-PrebuiltOrCargo -BinaryName 'h2o-native' -Features 'native-ui,embedded-runtime' `
        -BinaryArgs $binArgs.ToArray() -ForceCargo:$forceCargo -Online:$online
}

function Invoke-H2oRunUi {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $restArgs = ConvertTo-ArgArray $Rest
    $forceCargo = $false
    $online = $false
    $hasPort = $false
    $hasAssets = $false
    $wantsHelp = $false
    $binArgs = [System.Collections.Generic.List[string]]::new()

    for ($i = 0; $i -lt $restArgs.Count; $i++) {
        $arg = $restArgs[$i]
        switch -Regex ($arg) {
            '^(-Cargo|--cargo)$' { $forceCargo = $true }
            '^(-Online|--online)$' { $online = $true }
            '^(-h|--help)$' {
                $wantsHelp = $true
                $binArgs.Add($arg)
            }
            '^(-Port|--port)$' {
                $hasPort = $true
                $binArgs.Add('--port')
                $binArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            '^(-Assets|--assets)$' {
                $hasAssets = $true
                $binArgs.Add('--assets')
                $binArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            '^(-Snapshot|--snapshot)$' {
                $binArgs.Add('--snapshot')
                $binArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            default {
                $binArgs.Add($arg)
            }
        }
    }

    if (-not $wantsHelp) {
        if (-not $hasPort) {
            $binArgs.Add('--port')
            $binArgs.Add('8787')
        }
        if (-not $hasAssets) {
            $binArgs.Add('--assets')
            $binArgs.Add($DefaultUiAssets)
        }
    }

    Invoke-PrebuiltOrCargo -BinaryName 'h2o-ui' -Features 'ui' `
        -BinaryArgs $binArgs.ToArray() -ForceCargo:$forceCargo -Online:$online
}

function Get-DefaultApiSocketPath {
    if (-not [string]::IsNullOrEmpty($env:JCODE_API_SOCKET)) {
        return $env:JCODE_API_SOCKET
    }
    $runtimeDir = if (-not [string]::IsNullOrEmpty($env:JCODE_RUNTIME_DIR)) {
        $env:JCODE_RUNTIME_DIR
    } elseif (-not [string]::IsNullOrEmpty($env:XDG_RUNTIME_DIR)) {
        $env:XDG_RUNTIME_DIR
    } else {
        $rawUser = if (-not [string]::IsNullOrEmpty($env:USERNAME)) { $env:USERNAME } elseif (-not [string]::IsNullOrEmpty($env:USER)) { $env:USER } else { 'user' }
        $safeUser = ($rawUser -replace '[^A-Za-z0-9\-_]', '')
        if ($safeUser.Length -gt 64) { $safeUser = $safeUser.Substring(0, 64) }
        if ([string]::IsNullOrEmpty($safeUser)) { $safeUser = 'user' }
        Join-Path ([System.IO.Path]::GetTempPath()) "jcode-$safeUser"
    }
    return Join-Path $runtimeDir 'jcode-api.sock'
}

function Invoke-H2oServeDaemon {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $restArgs = ConvertTo-ArgArray $Rest
    $jcode = Find-JcodeExecutable
    $wantsHelp = $false
    $serveArgs = [System.Collections.Generic.List[string]]::new()
    $serveArgs.Add('serve')

    for ($i = 0; $i -lt $restArgs.Count; $i++) {
        $arg = $restArgs[$i]
        switch -Regex ($arg) {
            '^(-h|--help)$' {
                $wantsHelp = $true
                $serveArgs.Add($arg)
            }
            '^(-Socket|--socket)$' {
                $serveArgs.Add('--socket')
                $serveArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            '^(-Provider|--provider|-p)$' {
                $serveArgs.Add('--provider')
                $serveArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            '^(-Model|--model|-m)$' {
                $serveArgs.Add('--model')
                $serveArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
            }
            default {
                $serveArgs.Add($arg)
            }
        }
    }

    if (-not $wantsHelp) {
        $defaultApiSocket = Get-DefaultApiSocketPath
        Write-Host "Starting jcode daemon via: $jcode $($serveArgs -join ' ')" -ForegroundColor Cyan
        Write-Host "[note] Default Harness API socket on this machine: $defaultApiSocket (override via JCODE_API_SOCKET / JCODE_RUNTIME_DIR)." -ForegroundColor Yellow
        Write-Host '[note] h2o-native (Real Chat) runs standalone with its embedded in-process runtime (leave endpoint blank), or connects to an external Harness API bridge (`jcode-harness-api-bridge` or `jcode api-bridge`) alongside `jcode serve`.' -ForegroundColor Yellow
    }

    & $jcode @($serveArgs.ToArray())
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

function Invoke-H2oRunMonitor {
    param(
        [AllowEmptyCollection()]
        [string[]]$Rest = @()
    )

    $restArgs = ConvertTo-ArgArray $Rest
    $forceCargo = $false
    $online = $false
    $wantsHelp = $false
    $binArgs = [System.Collections.Generic.List[string]]::new()

    $valueOptions = @{
        '-Socket'          = '--socket'
        '--socket'         = '--socket'
        '-Spec'            = '--spec'
        '--spec'           = '--spec'
        '-Journal'         = '--journal'
        '--journal'        = '--journal'
        '-Sources'         = '--sources'
        '--sources'        = '--sources'
        '-Owner'           = '--owner'
        '--owner'          = '--owner'
        '-Worker'          = '--worker'
        '--worker'         = '--worker'
        '-JevBaseUrl'      = '--jev-base-url'
        '--jev-base-url'   = '--jev-base-url'
        '-MinProbability'  = '--min-probability'
        '--min-probability'= '--min-probability'
        '-DeadlineMs'      = '--deadline-ms'
        '--deadline-ms'    = '--deadline-ms'
        '-OutputCapBytes'  = '--output-cap-bytes'
        '--output-cap-bytes'= '--output-cap-bytes'
        '-PollMs'          = '--poll-ms'
        '--poll-ms'        = '--poll-ms'
    }

    for ($i = 0; $i -lt $restArgs.Count; $i++) {
        $arg = $restArgs[$i]
        if ($arg -eq '-Cargo' -or $arg -eq '--cargo') {
            $forceCargo = $true
        } elseif ($arg -eq '-Online' -or $arg -eq '--online') {
            $online = $true
        } elseif ($arg -eq '-h' -or $arg -eq '--help') {
            $wantsHelp = $true
            $binArgs.Add($arg)
        } elseif ($arg -eq '-EnableJev' -or $arg -eq '--enable-jev') {
            $binArgs.Add('--enable-jev')
        } elseif ($arg -eq '-IncludeContextBytes' -or $arg -eq '--include-context-bytes') {
            $binArgs.Add('--include-context-bytes')
        } elseif ($valueOptions.ContainsKey($arg)) {
            $binArgs.Add($valueOptions[$arg])
            $binArgs.Add((Read-OptionValue -Items $restArgs -Index ([ref]$i) -OptionName $arg))
        } else {
            $binArgs.Add($arg)
        }
    }

    if (-not $wantsHelp -and [string]::IsNullOrEmpty($env:TYPESAFE_API_KEY)) {
        Write-Host '[note] TYPESAFE_API_KEY is not set; live Jev requests require TYPESAFE_API_KEY in your environment.' -ForegroundColor Yellow
    }

    Invoke-PrebuiltOrCargo -BinaryName 'h2o-monitor' -Features 'sdk jev' `
        -BinaryArgs $binArgs.ToArray() -ForceCargo:$forceCargo -Online:$online
}

$command = if ($args.Count -gt 0) { [string]$args[0] } else { 'help' }
$restParam = if ($args.Count -gt 1) { @{ Rest = [string[]]$args[1..($args.Count - 1)] } } else { @{} }

switch ($command.ToLowerInvariant()) {
    { $_ -in @('help', '-h', '--help', '-help', '/?') } { Show-H2oHelp }
    'test'          { Invoke-H2oTest @restParam }
    'clippy'        { Invoke-H2oClippy @restParam }
    'build-release' { Invoke-H2oBuildRelease @restParam }
    'run-native'    { Invoke-H2oRunNative @restParam }
    'run-ui'        { Invoke-H2oRunUi @restParam }
    'serve-daemon'  { Invoke-H2oServeDaemon @restParam }
    'run-monitor'   { Invoke-H2oRunMonitor @restParam }
    default {
        [Console]::Error.WriteLine("Unknown command '$command'. Run '.\scripts\h2o.ps1 help' for usage.")
        exit 2
    }
}
