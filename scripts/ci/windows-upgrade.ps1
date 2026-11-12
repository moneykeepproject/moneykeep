<#
  upgrade-resilience-1 Batch U3: one upgrade scenario of the Windows CI job
  (.github/workflows/windows.yml), run as administrator on a GitHub-hosted runner.

  -Scenario pre-a1           a stand-in old service (LocalSystem, data in this user's
                             %LOCALAPPDATA%, plain inbox.key, an inbox item and a Wi-Fi
                             certificate), then Setup /S; checks the service account and
                             folder, health, the inbox item, the CA fingerprint and
                             moved.json in the old folder.
  -Scenario custom-folder    the same with Setup /S /D=C:\MK (H1).
  -Scenario reinstall        after custom-folder: uninstall keeping the data, Setup
                             again, then the helper's "Install the server again" plan.
  -Scenario previous-release the previous release's setup from its GitHub release asset,
                             capture set up with its own programs, then this Setup;
                             skipped with a notice while no release has a setup.

  Needs `cargo xtask installer` (dist\) and `cargo build -p moneykeep-server --example
  ci_fixture` (target\debug\examples\ci_fixture.exe) first. The scenario's logs go to
  $Artifacts\<scenario> (uploaded by the workflow).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('pre-a1', 'custom-folder', 'reinstall', 'previous-release')]
    [string]$Scenario,
    [string]$Work = (Join-Path $env:RUNNER_TEMP 'mk-upgrade'),
    [string]$Artifacts = (Join-Path $env:RUNNER_TEMP 'mk-artifacts')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Service = 'MoneyKeepCapture'
$Account = 'NT SERVICE\MoneyKeepCapture'
$Url = 'http://127.0.0.1:8787'
$MachineDir = Join-Path $env:ProgramData 'MoneyKeep'
$MachineData = Join-Path $MachineDir 'capture-server'
$UserData = Join-Path $env:LOCALAPPDATA 'MoneyKeep\data\capture-server'
$CopyDir = Join-Path $env:ProgramFiles 'MoneyKeep\capture'
$UninstallKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\MoneyKeep'
$Rule = 'MoneyKeep capture (Wi-Fi)'
$Fixture = Join-Path $Root 'target\debug\examples\ci_fixture.exe'
$FixtureFile = Join-Path $Work 'fixture.json'
$Out = Join-Path $Artifacts $Scenario

function Fail([string]$why) { throw "FAILED ($Scenario): $why" }

function Get-One([string]$pattern, [switch]$Directory) {
    $found = @(Get-ChildItem -Path (Join-Path $Root 'dist') -Filter $pattern -Directory:$Directory)
    if ($found.Count -ne 1) { Fail "expected one dist\$pattern, found $($found.Count): run cargo xtask installer" }
    return $found[0].FullName
}

function Get-Capture { Get-CimInstance -ClassName Win32_Service -Filter "Name='$Service'" }

function Get-InstallDir {
    $k = Get-ItemProperty -Path $UninstallKey -ErrorAction SilentlyContinue
    if ($k -and $k.PSObject.Properties['InstallLocation']) { return $k.InstallLocation }
    return $null
}

function Invoke-Setup([string]$exe, [string[]]$more = @()) {
    $argv = @('/S') + $more
    Write-Host "==> $exe $($argv -join ' ')"
    $p = Start-Process -FilePath $exe -ArgumentList $argv -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "$exe exited with $($p.ExitCode)" }
}

function Invoke-Uninstall {
    $dir = Get-InstallDir
    if (-not $dir) { Fail 'MoneyKeep is not installed' }
    $exe = Join-Path $dir 'uninstall.exe'
    Write-Host "==> $exe /S (keeps the capture server's data)"
    # _?= runs it in place, so Start-Process waits for it.
    $p = Start-Process -FilePath $exe -ArgumentList @('/S', "_?=$dir") -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "the uninstaller exited with $($p.ExitCode)" }
}

# The helper's plan, as the desktop writes and approves it (services apply).
function Invoke-Plan([string]$server, [string]$json, [string]$name) {
    $plan = Join-Path $Work "$name-plan.json"
    $report = Join-Path $Out "$name-report.json"
    Set-Content -Path $plan -Value $json -NoNewline -Encoding ascii
    $sha = (Get-FileHash -Path $plan -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "==> $server services apply ($json)"
    & $server services apply --plan $plan --report $report --plan-sha256 $sha
    $code = $LASTEXITCODE
    if (Test-Path $report) { Get-Content -Path $report -Raw | Write-Host }
    if ($code -ne 0) { Fail "the helper's plan $json failed (exit $code)" }
}

# Nothing of MoneyKeep left on the runner.
function Reset-Machine {
    if (Get-InstallDir) { Invoke-Uninstall }
    if (Get-Capture) {
        Stop-Service -Name $Service -Force -ErrorAction SilentlyContinue
        & sc.exe delete $Service | Out-Null
        Start-Sleep -Seconds 2
    }
    Remove-NetFirewallRule -DisplayName $Rule -ErrorAction SilentlyContinue
    foreach ($d in @($MachineDir, (Join-Path $env:LOCALAPPDATA 'MoneyKeep'),
            (Join-Path $env:ProgramFiles 'MoneyKeep'), 'C:\MK')) {
        if (Test-Path $d) { Remove-Item -Path $d -Recurse -Force }
    }
    Remove-Item -Path $UninstallKey -Recurse -ErrorAction SilentlyContinue
    if (Test-Path $FixtureFile) { Remove-Item $FixtureFile }
}

# A server from before security-review-1 A1: its program in the service's own folder
# (bytes added, so it differs from this build's), LocalSystem, data in this user's
# profile. Registered but not started: this build's program would seal the plain key
# on starting, and the upgrade must meet it plain.
function New-StandIn {
    New-Item -ItemType Directory -Force -Path $CopyDir | Out-Null
    $prog = Join-Path $CopyDir 'moneykeep-server.exe'
    Copy-Item -Path $Server -Destination $prog -Force
    $extra = [Text.Encoding]::ASCII.GetBytes('stand-in for an older program')
    $fs = [IO.File]::Open($prog, [IO.FileMode]::Append, [IO.FileAccess]::Write)
    try { $fs.Write($extra, 0, $extra.Length) } finally { $fs.Close() }
    & $Fixture pre-a1 --data $UserData --out $FixtureFile
    if ($LASTEXITCODE -ne 0) { Fail 'making the pre-A1 data folder failed' }
    if (-not (Test-Path (Join-Path $UserData 'inbox.key'))) { Fail 'the stand-in has no plain inbox.key' }
    New-Service -Name $Service -DisplayName 'MoneyKeep phone capture server' -StartupType Automatic `
        -BinaryPathName "`"$prog`" service-run --data `"$UserData`" --key-file" | Out-Null
    $s = Get-Capture
    if ($s.StartName -ne 'LocalSystem') { Fail "the stand-in runs as $($s.StartName)" }
    Write-Host "stand-in old service: $($s.StartName) $($s.PathName)"
}

# The service runs as its own account from the protected folder, passes its health
# check, opens the fixture's inbox item and keeps its Wi-Fi certificate authority.
function Assert-Server {
    $s = Get-Capture
    if (-not $s) { Fail "the service $Service is not installed" }
    Write-Host "service: $($s.StartName) $($s.State) $($s.PathName)"
    if ($s.StartName -ne $Account) { Fail "the service runs as '$($s.StartName)', not $Account" }
    if ($s.PathName -notlike "*--data*$MachineData*") {
        Fail "the service's data folder is not $MachineData : $($s.PathName)"
    }
    $deadline = (Get-Date).AddSeconds(60)
    while ($true) {
        $s = Get-Capture
        if ($s -and $s.State -eq 'Running') { break }
        if ((Get-Date) -gt $deadline) { Fail "the service is not running ($(if ($s) { $s.State }))" }
        Start-Sleep -Seconds 1
    }
    & $Fixture check --url $Url --fixture $FixtureFile --wait 60
    if ($LASTEXITCODE -ne 0) { Fail 'the health check, the inbox item or the CA fingerprint check failed' }
    if (Test-Path (Join-Path $MachineData 'inbox.key')) { Fail 'a plain inbox.key is in the protected folder' }
    if (-not (Test-Path (Join-Path $MachineData 'inbox.key.sealed'))) { Fail 'no inbox.key.sealed in the protected folder' }
}

# After the move: the old folder is marked, and the service's program is this build's (H1).
function Assert-Moved {
    if (-not (Test-Path (Join-Path $UserData 'moved.json'))) { Fail "no moved.json in $UserData" }
    if (Test-Path (Join-Path $UserData 'server.toml')) { Fail "$UserData still has server.toml" }
    $prog = Join-Path $CopyDir 'moneykeep-server.exe'
    if ((Get-FileHash $prog).Hash -ne (Get-FileHash $Server).Hash) { Fail "$prog is not this build's program" }
    if ((Get-Capture).PathName -notlike "*$prog*") { Fail "the service does not run $prog" }
}

function Save-Logs {
    New-Item -ItemType Directory -Force -Path $Out | Out-Null
    foreach ($f in @('server.log', 'server.log.1')) {
        $p = Join-Path $MachineData $f
        if (Test-Path $p) { Copy-Item -Path $p -Destination $Out }
    }
    $p = Join-Path $UserData 'moved.json'
    if (Test-Path $p) { Copy-Item -Path $p -Destination $Out }
    $s = Get-Capture
    if ($s) {
        $s | Select-Object Name, State, StartName, PathName, ExitCode, ServiceSpecificExitCode |
            Format-List | Out-File (Join-Path $Out 'service.txt')
    }
    $dir = Get-InstallDir
    if ($dir -and (Test-Path (Join-Path $dir 'moneykeep-server.exe'))) {
        & (Join-Path $dir 'moneykeep-server.exe') services status 2>&1 |
            Out-File (Join-Path $Out 'services-status.json')
    }
    # The silent setup drops move-data's report. When the service still isn't on the
    # protected folder, run it once more by hand from the same start (no machine folder:
    # a folder the first run left behind would only be refused as not MoneyKeep's), so
    # the report says why, and show the folder's owner and permissions afterwards.
    if ($s -and $dir -and $s.PathName -notlike "*$MachineData*" -and
            (Test-Path (Join-Path $dir 'moneykeep-server.exe'))) {
        if (Test-Path $MachineDir) { Remove-Item -Path $MachineDir -Recurse -Force }
        & (Join-Path $dir 'moneykeep-server.exe') services move-data 2>&1 |
            Out-File (Join-Path $Out 'move-data-again.json')
        foreach ($d in @($MachineDir, $MachineData)) {
            if (Test-Path $d) {
                "== $d" | Out-File -Append (Join-Path $Out 'acl.txt')
                Get-Acl -LiteralPath $d | Format-List Owner, AccessToString, AreAccessRulesProtected |
                    Out-File -Append (Join-Path $Out 'acl.txt')
            }
        }
    }
    # Also in the job's own log, so a failure reads without downloading the artifact.
    foreach ($f in @(Get-ChildItem -Path $Out -File | Where-Object { $_.Name -ne 'steps.log' })) {
        Write-Host "----- $($f.Name)"
        Get-Content -Path $f.FullName -Tail 80 | ForEach-Object { Write-Host $_ }
    }
}

function Test-PreA1 {
    Reset-Machine
    New-StandIn
    Invoke-Setup $Setup
    Assert-Server
    Assert-Moved
}

function Test-CustomFolder {
    Reset-Machine
    New-StandIn
    Invoke-Setup $Setup @('/D=C:\MK')
    if ((Get-InstallDir) -ne 'C:\MK') { Fail "installed in $(Get-InstallDir), not C:\MK" }
    if (-not (Test-Path 'C:\MK\moneykeep-app.exe')) { Fail 'C:\MK has no moneykeep-app.exe' }
    Assert-Server
    Assert-Moved
}

function Test-Reinstall {
    if (-not (Get-Capture) -or -not (Test-Path $FixtureFile)) { Fail 'run the custom-folder scenario first' }
    Invoke-Uninstall
    if (Get-Capture) { Fail 'the service is still installed after uninstalling' }
    if (-not (Test-Path (Join-Path $MachineData 'server.toml'))) { Fail "the uninstaller removed $MachineData" }
    Invoke-Setup $Setup
    if (Get-Capture) { Fail 'Setup installed the service itself; this scenario expects the helper to' }
    $dir = Get-InstallDir
    Invoke-Plan (Join-Path $dir 'moneykeep-server.exe') '{"reinstall_server":true}' 'reinstall'
    Assert-Server
}

function Test-PreviousRelease {
    $repo = $env:GITHUB_REPOSITORY
    if (-not $repo) { Fail 'GITHUB_REPOSITORY is not set' }
    $version = (Get-Content -Path (Join-Path $Dist 'version.json') -Raw | ConvertFrom-Json).version
    $json = gh api "repos/$repo/releases?per_page=30"
    if ($LASTEXITCODE -ne 0) { Fail 'listing the releases failed' }
    $pick = $null
    foreach ($r in @(($json -join "`n") | ConvertFrom-Json)) {
        if ($r.draft -or $r.tag_name -eq $version -or $r.tag_name -eq "v$version") { continue }
        $asset = @($r.assets | Where-Object { $_.name -like 'moneykeep-*-windows-x64-setup.exe' }) | Select-Object -First 1
        if ($asset) { $pick = @{ tag = $r.tag_name; name = $asset.name }; break }
    }
    if (-not $pick) {
        Write-Host '::notice title=Previous release::No earlier release has a Windows setup yet; upgrading from the previous release was skipped.'
        return
    }
    $prev = Join-Path $Work 'previous'
    New-Item -ItemType Directory -Force -Path $prev | Out-Null
    gh release download $pick.tag --repo $repo --pattern $pick.name --dir $prev --clobber
    if ($LASTEXITCODE -ne 0) { Fail "downloading $($pick.name) from $($pick.tag) failed" }
    Reset-Machine
    Invoke-Setup (Join-Path $prev $pick.name)
    # Phone capture set up with the previous release's own programs.
    $old = Join-Path (Get-InstallDir) 'moneykeep-server.exe'
    $init = & $old init --data $MachineData --key-file --json
    if ($LASTEXITCODE -ne 0) { Fail "$old init failed" }
    $token = ($init | ConvertFrom-Json).desktop_token
    $toml = Join-Path $MachineData 'server.toml'
    $text = (Get-Content -Path $toml -Raw).Replace('access_required = true', 'access_required = false')
    Set-Content -Path $toml -Value $text -NoNewline
    & $old wifi configure --data $MachineData --address 127.0.0.1 --key-file --computer CI
    if ($LASTEXITCODE -ne 0) { Fail "$old wifi configure failed" }
    Invoke-Plan $old '{"reinstall_server":true}' 'previous-install'
    & $Fixture phone --url $Url --token $token --out $FixtureFile
    if ($LASTEXITCODE -ne 0) { Fail "pairing a phone with $($pick.tag)'s server failed" }
    Invoke-Setup $Setup
    Assert-Server
}

New-Item -ItemType Directory -Force -Path $Work, $Out | Out-Null
Start-Transcript -Path (Join-Path $Out 'steps.log') | Out-Null
try {
    if (-not (Test-Path $Fixture)) { Fail "$Fixture is missing: cargo build -p moneykeep-server --example ci_fixture" }
    $Setup = Get-One 'moneykeep-*-windows-x64-setup.exe'
    $Dist = Get-One 'MoneyKeep-*' -Directory
    $Server = Join-Path $Dist 'moneykeep-server.exe'
    switch ($Scenario) {
        'pre-a1' { Test-PreA1 }
        'custom-folder' { Test-CustomFolder }
        'reinstall' { Test-Reinstall }
        'previous-release' { Test-PreviousRelease }
    }
    Write-Host "PASSED ($Scenario)"
}
finally {
    Save-Logs
    Stop-Transcript | Out-Null
}
