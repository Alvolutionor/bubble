param([string]$Project, [string]$Cwd)

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
}
"@

$script:found = [IntPtr]::Zero
$suffix = "$Project - Visual Studio Code"
$cb = [W+EnumProc]{
  param($h, $l)
  if (-not [W]::IsWindowVisible($h)) { return $true }
  $len = [W]::GetWindowTextLength($h)
  if ($len -le 0) { return $true }
  $sb = New-Object System.Text.StringBuilder ($len + 1)
  [void][W]::GetWindowText($h, $sb, $sb.Capacity)
  $t = $sb.ToString()
  if ($t.EndsWith($suffix)) { $script:found = $h; return $false }
  return $true
}
[void][W]::EnumWindows($cb, [IntPtr]::Zero)

if ($script:found -ne [IntPtr]::Zero) {
  $h = $script:found
  if ([W]::IsIconic($h)) { [void][W]::ShowWindow($h, 9) }
  $fg = [W]::GetForegroundWindow()
  $procId = 0
  $t1 = [W]::GetWindowThreadProcessId($fg, [ref]$procId)
  $t2 = [W]::GetWindowThreadProcessId($h, [ref]$procId)
  $cur = [W]::GetCurrentThreadId()
  [void][W]::AttachThreadInput($cur, $t1, $true)
  [void][W]::AttachThreadInput($cur, $t2, $true)
  [void][W]::BringWindowToTop($h)
  [void][W]::SetForegroundWindow($h)
  [void][W]::AttachThreadInput($cur, $t1, $false)
  [void][W]::AttachThreadInput($cur, $t2, $false)
  Write-Output "focused"
} else {
  Start-Process code -ArgumentList @('-n', $Cwd)
  Write-Output "opened"
}
