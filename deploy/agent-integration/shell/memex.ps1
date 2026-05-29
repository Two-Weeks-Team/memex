# Memex · PowerShell integration — primer on directory change.
#   . C:\path\to\deploy\agent-integration\shell\memex.ps1   # dot-source in $PROFILE
# Fail-open; human-facing. Idempotent on re-source; chains through any existing prompt
# instead of clobbering it.
if (-not $Global:_MemexInstalled) {
    $Global:_MemexInstalled = $true
    $Global:_MemexLastPwd = ""
    # Capture any existing prompt so we chain through it rather than replacing it.
    $Global:_MemexOriginalPrompt = if (Test-Path Function:\prompt) { (Get-Command prompt).ScriptBlock } else { $null }
    function global:prompt {
        if ($PWD.Path -ne $Global:_MemexLastPwd) {
            $Global:_MemexLastPwd = $PWD.Path
            if (Get-Command memex -ErrorAction SilentlyContinue) {
                if ((Test-Path "$($PWD.Path)\.git") -or (Test-Path "$($PWD.Path)\.claude")) {
                    try { memex memory --cwd "$($PWD.Path)" --hook shell 2>$null } catch {}
                }
            }
        }
        if ($Global:_MemexOriginalPrompt) { & $Global:_MemexOriginalPrompt }
        else { "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) " }
    }
}
