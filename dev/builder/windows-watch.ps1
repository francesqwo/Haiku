# builder: watches a running Haiku Setup and every process it started for
# windows. Every second until the setup exits it records how many processes
# the setup has started (gcc, cc1, as, ar, ld, cmd), the console hosts among
# them (conhost.exe, OpenConsole.exe, WindowsTerminal.exe, by name), and the
# visible top-level windows any of them owns (EnumWindows + IsWindowVisible,
# the setup's own included). Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File dev/builder/windows-watch.ps1 <setup pid> <out.tsv>
param([int]$SetupPid, [string]$Out)
Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Wins {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public static List<uint> VisibleOwners() {
        var owners = new List<uint>();
        EnumWindows((h, l) => { if (IsWindowVisible(h)) { uint pid; GetWindowThreadProcessId(h, out pid); owners.Add(pid); } return true; }, IntPtr.Zero);
        return owners;
    }
}
"@
"time`tchildren`tgcc`tcc1`tconhost`topenconsole`twindowsterminal`tvisible_windows`twindow_owners" | Out-File -Encoding utf8 $Out
$max = @{ children = 0; conhost = 0; openconsole = 0; terminal = 0; windows = 0 }
$samples = 0; $hostLines = @{}
while (Get-Process -Id $SetupPid -ErrorAction SilentlyContinue) {
    $all = Get-CimInstance Win32_Process
    $tree = @{}; $tree[$SetupPid] = $true
    do {
        $added = $false
        foreach ($p in $all) {
            if (-not $tree.ContainsKey([int]$p.ProcessId) -and $tree.ContainsKey([int]$p.ParentProcessId)) { $tree[[int]$p.ProcessId] = $true; $added = $true }
        }
    } while ($added)
    $inTree = @($all | Where-Object { $tree.ContainsKey([int]$_.ProcessId) -and [int]$_.ProcessId -ne $SetupPid })
    $conhost = @($inTree | Where-Object { $_.Name -eq 'conhost.exe' })
    $openconsole = @($inTree | Where-Object { $_.Name -eq 'OpenConsole.exe' })
    $terminal = @($all | Where-Object { $_.Name -eq 'WindowsTerminal.exe' -and $tree.ContainsKey([int]$_.ParentProcessId) })
    foreach ($c in $conhost) { $hostLines[$c.CommandLine] = $true }
    $owners = @([Wins]::VisibleOwners() | Where-Object { $tree.ContainsKey([int]$_) })
    $names = @($owners | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName })
    $gcc = @($inTree | Where-Object { $_.Name -eq 'gcc.exe' }).Count
    $cc1 = @($inTree | Where-Object { $_.Name -eq 'cc1.exe' }).Count
    "$(Get-Date -Format HH:mm:ss)`t$($inTree.Count)`t$gcc`t$cc1`t$($conhost.Count)`t$($openconsole.Count)`t$($terminal.Count)`t$($owners.Count)`t$($names -join ',')" | Out-File -Encoding utf8 -Append $Out
    $samples++
    foreach ($k in @(@('children', $inTree.Count), @('conhost', $conhost.Count), @('openconsole', $openconsole.Count), @('terminal', $terminal.Count), @('windows', $owners.Count))) {
        if ($k[1] -gt $max[$k[0]]) { $max[$k[0]] = $k[1] }
    }
    Start-Sleep -Milliseconds 1000
}
"summary: $samples samples; most at once: processes started $($max.children), conhost $($max.conhost), OpenConsole $($max.openconsole), WindowsTerminal $($max.terminal), visible windows $($max.windows)" | Out-File -Encoding utf8 -Append $Out
foreach ($l in $hostLines.Keys) { "conhost command line: $l" | Out-File -Encoding utf8 -Append $Out }
