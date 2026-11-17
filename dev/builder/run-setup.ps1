# builder: runs Haiku Setup from a release folder with a script (its window
# hidden unless -Visible), watches its processes for console windows
# (windows-watch.ps1), and waits for it to end.
#   powershell -NoProfile -ExecutionPolicy Bypass -File dev/builder/run-setup.ps1 <folder> <shots> <script> <watch.tsv> [<jobs>] [-Visible]
param([string]$Folder, [string]$Shots, [string]$Script, [string]$Watch, [int]$Jobs = 16, [switch]$Visible)
$env:HAIKU_SETUP_JOBS = "$Jobs"
$argList = @('--shots', "`"$Shots`"", '--script', "`"$Script`"")
if (-not $Visible) { $argList = @('--hidden') + $argList }
$exe = Join-Path $Folder 'Haiku Setup.exe'
$start = Get-Date
$p = Start-Process -FilePath $exe -ArgumentList $argList -WorkingDirectory $Folder -PassThru
$watcher = Start-Process -FilePath powershell -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', "`"$PSScriptRoot\windows-watch.ps1`"", $p.Id, "`"$Watch`"") -WindowStyle Hidden -PassThru
$p.WaitForExit()
$watcher.WaitForExit()
"setup pid $($p.Id) exit $($p.ExitCode) after $([int]((Get-Date) - $start).TotalSeconds) s"
