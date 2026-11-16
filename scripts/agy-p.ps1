<#
.SYNOPSIS
  Direct interactive-shell helper to run official `agy -p` inside `jcode-h2o`
  in compliance with Google Antigravity Terms of Service.
#>
[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Position = 0)]
    [string]$Prompt,

    [string]$Model,
    [ValidateSet("low", "medium", "high", "xhigh", "max")]
    [string]$Effort,
    [ValidateSet("text", "json", "stream-json")]
    [string]$OutputFormat = "text",
    [switch]$Sandbox,
    [switch]$Help,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($Help -or $Prompt -eq "--help" -or $Prompt -eq "-h" -or ($ExtraArgs -and ($ExtraArgs -contains "--help" -or $ExtraArgs -contains "-h")) -or (-not $Prompt -and (-not $ExtraArgs -or $ExtraArgs.Count -eq 0))) {
    Write-Host @"
Usage: .\scripts\agy-p.ps1 "<prompt>" [-Model <model>] [-Effort <low|medium|high|xhigh|max>] [-OutputFormat <text|json|stream-json>] [-Sandbox] [-- <extra agy flags>]

Google Antigravity ToS compliance:
  - Run ONLY directly from your own interactive shell.
  - Refuses execution if an external coding agent environment (jcode / Claude Code / Codex / Gemini CLI / OpenCode) is detected.
  - Invokes the unmodified official agy.exe directly without OAuth proxies or third-party wrappers.
"@
    exit 0
}

$agentEnvMarkers = @(
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_THREAD_ID",
    "CODEX_CI",
    "GEMINI_CLI",
    "JCODE_SESSION",
    "JCODE_SOCKET",
    "JCODE_API_SOCKET",
    "OPENCODE"
)

foreach ($var in $agentEnvMarkers) {
    $val = [Environment]::GetEnvironmentVariable($var)
    if (-not [string]::IsNullOrEmpty($val)) {
        [Console]::Error.WriteLine("Blocked by scripts/agy-p.ps1: external agent environment detected ($var). Do not invoke 'agy -p' via external agents (Antigravity ToS compliance).")
        exit 2
    }
}

$agyCmd = Get-Command agy.exe -ErrorAction SilentlyContinue
if (-not $agyCmd) {
    $fallback = Join-Path $env:LOCALAPPDATA "agy\bin\agy.exe"
    if (Test-Path $fallback) {
        $agyPath = $fallback
    } else {
        [Console]::Error.WriteLine("Error: agy.exe not found on PATH or at $fallback")
        exit 1
    }
} else {
    $agyPath = $agyCmd.Source
}

$argsList = [System.Collections.Generic.List[string]]::new()
$argsList.Add("-p")
if ($Prompt) {
    $argsList.Add($Prompt)
}
if ($Model) {
    $argsList.Add("--model")
    $argsList.Add($Model)
}
if ($Effort) {
    $argsList.Add("--effort")
    $argsList.Add($Effort)
}
if ($OutputFormat) {
    $argsList.Add("--output-format")
    $argsList.Add($OutputFormat)
}
if ($Sandbox) {
    $argsList.Add("--sandbox")
}
if ($ExtraArgs) {
    foreach ($a in $ExtraArgs) {
        if ($a -ne "--") {
            $argsList.Add($a)
        }
    }
}

& $agyPath @argsList
exit $LASTEXITCODE
