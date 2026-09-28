param(
    [string]$Executable = (Join-Path $PSScriptRoot '..\target\x86_64-pc-windows-msvc\release\notepad.exe')
)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class NativeNotepadTest {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendNumber(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder b, int n);
    [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)] public static extern IntPtr ReadText(IntPtr h, uint m, IntPtr w, StringBuilder b);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetMenuState(IntPtr menu, uint id, uint flags);
    public static uint BackgroundColor(IntPtr window) {
        // EM_SETBKGNDCOLOR returns the previous color; restore it without pumping paint messages.
        IntPtr previous = SendNumber(window, 0x443, IntPtr.Zero, IntPtr.Zero);
        SendNumber(window, 0x443, IntPtr.Zero, previous);
        return (uint)previous.ToInt64();
    }
}
'@
$root = Join-Path (Split-Path $PSScriptRoot) ('target\smoke-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$fixture = Join-Path $root 'mixed.txt'
[IO.File]::WriteAllText($fixture, "first`r`nsecond`nthird`rlast", [Text.UTF8Encoding]::new($false))
$processes = [Collections.Generic.List[Diagnostics.Process]]::new()
function Start-Editor([string]$PathArgument) {
    $start = [Diagnostics.ProcessStartInfo]::new((Resolve-Path $Executable).Path)
    $start.UseShellExecute = $false
    $start.Environment['LOCALAPPDATA'] = $root
    $start.Environment['PATH'] = Join-Path $env:SystemRoot 'System32'
    if ($PathArgument) { $start.ArgumentList.Add($PathArgument) }
    $p = [Diagnostics.Process]::Start($start)
    $processes.Add($p)
    return $p
}
function Wait-For([scriptblock]$Condition, [string]$Description) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $ready = $false
        try { $ready = & $Condition } catch [IO.IOException] {
            if (($_.Exception.HResult -band 0xFFFF) -notin @(32, 33)) { throw }
        }
        if ($ready) { return }
        if ($watch.Elapsed.TotalSeconds -gt 15) { throw "Timed out: $Description" }
        Start-Sleep -Milliseconds 50
    }
}
function Main-Window($p) {
    $p.Refresh()
    return $p.MainWindowHandle
}
function Active-Editor([IntPtr]$window) {
    $child = [IntPtr]::Zero
    while ($true) {
        $child = [NativeNotepadTest]::FindWindowEx($window, $child, 'RICHEDIT50W', $null)
        if ($child -eq [IntPtr]::Zero) { throw 'No visible editor' }
        if ([NativeNotepadTest]::IsWindowVisible($child)) { return $child }
    }
}
function Read-Editor([IntPtr]$editor) {
    $buffer = [Text.StringBuilder]::new(65536)
    [void][NativeNotepadTest]::ReadText($editor, 0xD, [IntPtr]$buffer.Capacity, $buffer)
    return $buffer.ToString().Replace("`r`n", "`n").Replace("`r", "`n")
}
function Command([IntPtr]$window, [int]$command) {
    [void][NativeNotepadTest]::PostMessage($window, 0x111, [IntPtr]$command, [IntPtr]::Zero)
}
function Read-File([string]$path) {
    $file = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    $reader = [IO.StreamReader]::new($file)
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}
try {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Editor $fixture
    Wait-For { (Main-Window $p) -ne [IntPtr]::Zero } 'initial window'
    $window = Main-Window $p
    foreach ($kind in @(0, 1)) {
        if ([NativeNotepadTest]::SendNumber($window, 0x7F, [IntPtr]$kind, [IntPtr]::Zero) -eq [IntPtr]::Zero) {
            throw "Missing application window icon (small/big: $kind)"
        }
    }
    $editor = Active-Editor $window
    $launchMs = $watch.ElapsedMilliseconds
    if ((Read-Editor $editor) -ne "first`nsecond`nthird`nlast") { throw 'Initial open lost text' }
    Command $window 103
    Start-Sleep -Milliseconds 250
    if ((Read-File $fixture) -ne "first`r`nsecond`nthird`rlast") { throw 'Unchanged save lost mixed endings' }
    $sessionPath = Join-Path $root 'RustNotepad\session.json'
    Command $window 410
    Wait-For { (([NativeNotepadTest]::GetMenuState([NativeNotepadTest]::GetMenu($window), 410, 0)) -band 8) -ne 0 } 'dark menu check'
    Wait-For { (Test-Path $sessionPath) -and ((Read-File $sessionPath | ConvertFrom-Json).settings.dark_mode) } 'dark preference persisted'
    $pixel = [NativeNotepadTest]::BackgroundColor($editor)
    if ($pixel -ne 0x413830) { throw ('Dark background mismatch: 0x{0:X8}' -f $pixel) }
    $state = Read-File $sessionPath | ConvertFrom-Json
    if ($state.documents[$state.active].dirty) { throw 'Dark mode dirtied a clean document' }
    if ((Read-Editor $editor) -ne "first`nsecond`nthird`nlast") { throw 'Dark mode changed text' }
    Command $window 410
    Wait-For { (([NativeNotepadTest]::GetMenuState([NativeNotepadTest]::GetMenu($window), 410, 0)) -band 8) -eq 0 } 'light menu check'
    if ([NativeNotepadTest]::BackgroundColor($editor) -eq 0x413830) { throw 'Light mode did not restore editor background' }
    Command $window 410
    Wait-For { (([NativeNotepadTest]::GetMenuState([NativeNotepadTest]::GetMenu($window), 410, 0)) -band 8) -ne 0 } 'dark enabled again'
    [void][NativeNotepadTest]::SendMessage($editor, 0xC, [IntPtr]::Zero, "first`rsecond`rthird`rlast!")
    Wait-For { (Read-Editor $editor).EndsWith('!') } 'native edit'
    Start-Sleep -Milliseconds 100
    Command $window 201
    Wait-For { (Read-Editor $editor) -eq "first`nsecond`nthird`nlast" } 'undo'
    Command $window 202
    Wait-For { (Read-Editor $editor).EndsWith('!') } 'redo'
    Command $window 103
    Wait-For { (Read-File $fixture).EndsWith('!') } 'save edited file'
    if ((Read-File $fixture) -ne "first`r`nsecond`nthird`rlast!") { throw 'Edited save lost mixed endings' }
    [void][NativeNotepadTest]::SendNumber($editor, 0xB1, [IntPtr](-1), [IntPtr](-1))
    [void][NativeNotepadTest]::SendNumber($editor, 0x102, [IntPtr]63, [IntPtr]::Zero)
    Wait-For { (Read-Editor $editor).EndsWith('!?') } 'incremental native typing'
    Command $window 201
    Wait-For { (Read-Editor $editor).EndsWith('!') } 'undo native typing'
    [void][NativeNotepadTest]::SendNumber($editor, 0x102, [IntPtr]0xD83D, [IntPtr]::Zero)
    [void][NativeNotepadTest]::SendNumber($editor, 0x102, [IntPtr]0xDE00, [IntPtr]::Zero)
    $emoji = [char]::ConvertFromUtf32(0x1F600)
    Wait-For { (Read-Editor $editor).EndsWith($emoji) } 'supplementary Unicode typing'
    Command $window 201
    Wait-For { (Read-Editor $editor).EndsWith('!') } 'undo supplementary Unicode'
    Command $window 101
    Start-Sleep -Milliseconds 200
    $editor = Active-Editor $window
    [void][NativeNotepadTest]::SendMessage($editor, 0xC, [IntPtr]::Zero, 'unsaved recovery sample')
    Command $window 409
    $sessionPath = Join-Path $root 'RustNotepad\session.json'
    Wait-For { (Test-Path $sessionPath) -and (Read-File $sessionPath).Contains('unsaved recovery sample') } 'checkpoint'
    [void][NativeNotepadTest]::PostMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $p.WaitForExit(15000)) { throw 'Orderly exit failed' }
    $p = Start-Editor ''
    Wait-For { (Main-Window $p) -ne [IntPtr]::Zero } 'restored window'
    $window = Main-Window $p
    $editor = Active-Editor $window
    if ((Read-Editor $editor) -ne 'unsaved recovery sample') { throw 'Unsaved tab did not restore' }
    if ([NativeNotepadTest]::BackgroundColor($editor) -ne 0x413830) { throw 'Dark appearance did not survive restart' }
    if ((([NativeNotepadTest]::GetMenuState([NativeNotepadTest]::GetMenu($window), 410, 0)) -band 8) -eq 0) { throw 'Dark menu preference did not survive restart' }
    [void][NativeNotepadTest]::SendMessage($editor, 0xC, [IntPtr]::Zero, 'crash recovery sample')
    Command $window 409
    Wait-For { (Read-File $sessionPath).Contains('crash recovery sample') } 'crash checkpoint'
    Stop-Process -Id $p.Id
    $p.WaitForExit()
    $p = Start-Editor ''
    Wait-For { (Main-Window $p) -ne [IntPtr]::Zero } 'crash restored window'
    $window = Main-Window $p
    $editor = Active-Editor $window
    if ((Read-Editor $editor) -ne 'crash recovery sample') { throw 'Crash checkpoint did not restore' }
    $secondFile = Join-Path $root 'forwarded.txt'
    [IO.File]::WriteAllText($secondFile, 'forwarded file')
    $second = Start-Editor $secondFile
    if (-not $second.WaitForExit(15000)) { throw 'Second instance did not forward and exit' }
    if ($second.ExitCode -ne 0) { throw 'Second instance returned an error' }
    Wait-For { (Read-Editor (Active-Editor $window)) -eq 'forwarded file' } 'IPC file forwarding'
    [void][NativeNotepadTest]::PostMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $p.WaitForExit(15000)) { throw 'Final exit failed' }
    $result = "PASS: dark/light colors, persisted theme, unchanged clean document, file open/save, mixed EOL, native editing, undo/redo, tabs, orderly restore, crash restore, instance forwarding. Launch-ready: ${launchMs}ms. Artifacts: $root"
    $result | Set-Content (Join-Path $root 'result.txt')
    Write-Output $result
} finally {
    foreach ($p in $processes) {
        if (-not $p.HasExited) { Stop-Process -Id $p.Id }
    }
}
