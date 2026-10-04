import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execute = promisify(execFile);

export async function inspectNativeTestClipboard(expected) {
  if (process.platform !== "win32") return { available: false };
  const command = `
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PromptifyClipboardDiagnostic {
  [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
  [DllImport("user32.dll")] public static extern IntPtr GetClipboardOwner();
  [DllImport("user32.dll")] public static extern IntPtr GetOpenClipboardWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
  [DllImport("user32.dll")] public static extern bool IsClipboardFormatAvailable(uint format);
  [DllImport("user32.dll")] public static extern bool OpenClipboard(IntPtr window);
  [DllImport("user32.dll")] public static extern bool CloseClipboard();
  [DllImport("user32.dll")] public static extern IntPtr GetClipboardData(uint format);
  [DllImport("kernel32.dll")] public static extern IntPtr GlobalLock(IntPtr memory);
  [DllImport("kernel32.dll")] public static extern bool GlobalUnlock(IntPtr memory);
}
'@
[uint32]$owner = 0
[uint32]$reader = 0
[void][PromptifyClipboardDiagnostic]::GetWindowThreadProcessId([PromptifyClipboardDiagnostic]::GetClipboardOwner(), [ref]$owner)
[void][PromptifyClipboardDiagnostic]::GetWindowThreadProcessId([PromptifyClipboardDiagnostic]::GetOpenClipboardWindow(), [ref]$reader)
$result = @{ sequence = [PromptifyClipboardDiagnostic]::GetClipboardSequenceNumber(); owner = $owner; reader = $reader; unicode_available = [PromptifyClipboardDiagnostic]::IsClipboardFormatAvailable(13) }
if (-not [PromptifyClipboardDiagnostic]::OpenClipboard([IntPtr]::Zero)) {
  $result.read_error = "The clipboard is occupied"
} else {
  try {
    $memory = [PromptifyClipboardDiagnostic]::GetClipboardData(13)
    if ($memory -eq [IntPtr]::Zero) { $result.read_error = "Unicode data is unavailable" }
    else {
      $pointer = [PromptifyClipboardDiagnostic]::GlobalLock($memory)
      if ($pointer -eq [IntPtr]::Zero) { $result.read_error = "Unicode data could not be locked" }
      else {
        try { $result.matches_expected = [System.Runtime.InteropServices.Marshal]::PtrToStringUni($pointer) -ceq $env:PROMPTIFY_EXPECTED_CLIPBOARD }
        finally { [void][PromptifyClipboardDiagnostic]::GlobalUnlock($memory) }
      }
    }
  } finally {
    if (-not [PromptifyClipboardDiagnostic]::CloseClipboard()) { throw "Could not close the diagnostic clipboard read" }
  }
}
$result | ConvertTo-Json -Compress
`;
  const { stdout } = await execute("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", command], {
    windowsHide: true, env: { ...process.env, PROMPTIFY_EXPECTED_CLIPBOARD: expected }, timeout: 10000,
  });
  return JSON.parse(stdout);
}

export async function focusNativeTest(page, title) {
  await page.bringToFront();
  if (process.platform !== "win32") return;
  if (!title.startsWith("Promptify paste test ")) throw new Error("Only native test windows may be activated.");
  const command = `
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PromptifyTestWindow {
  public delegate bool WindowCallback(IntPtr window, IntPtr data);
  [DllImport("user32.dll")] public static extern bool EnumWindows(WindowCallback callback, IntPtr data);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint source, uint target, bool attach);
}
'@
$matches = [System.Collections.Generic.List[IntPtr]]::new()
$callback = [PromptifyTestWindow+WindowCallback] {
  param($window, $data)
  $caption = [System.Text.StringBuilder]::new(2048)
  [void][PromptifyTestWindow]::GetWindowText($window, $caption, $caption.Capacity)
  if ([PromptifyTestWindow]::IsWindowVisible($window) -and $caption.ToString().Contains($env:PROMPTIFY_TEST_WINDOW)) {
    $matches.Add($window)
  }
  return $true
}
[void][PromptifyTestWindow]::EnumWindows($callback, [IntPtr]::Zero)
if ($matches.Count -ne 1) { throw "The unique native test window was not found." }
[void][PromptifyTestWindow]::ShowWindow($matches[0], 9)
[void][PromptifyTestWindow]::SetForegroundWindow($matches[0])
if ([PromptifyTestWindow]::GetForegroundWindow() -ne $matches[0]) {
  $shell = New-Object -ComObject WScript.Shell
  try {
    [void]$shell.AppActivate($env:PROMPTIFY_TEST_WINDOW)
  } finally {
    [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
  }
}
if ([PromptifyTestWindow]::GetForegroundWindow() -ne $matches[0]) {
  [uint32]$process = 0
  $foregroundThread = [PromptifyTestWindow]::GetWindowThreadProcessId([PromptifyTestWindow]::GetForegroundWindow(), [ref]$process)
  $thread = [PromptifyTestWindow]::GetCurrentThreadId()
  if ($foregroundThread -ne 0 -and $foregroundThread -ne $thread -and [PromptifyTestWindow]::AttachThreadInput($thread, $foregroundThread, $true)) {
    try {
      [void][PromptifyTestWindow]::SetForegroundWindow($matches[0])
    } finally {
      if (-not [PromptifyTestWindow]::AttachThreadInput($thread, $foregroundThread, $false)) {
        throw "Could not detach native test focus setup."
      }
    }
  }
}
for ($attempt = 0; $attempt -lt 600 -and [PromptifyTestWindow]::GetForegroundWindow() -ne $matches[0]; $attempt++) {
  Start-Sleep -Milliseconds 100
}
if ([PromptifyTestWindow]::GetForegroundWindow() -ne $matches[0]) {
  throw "Windows denied activation of the native test window. The test requires an available interactive desktop."
}
`;
  await execute("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", command], {
    windowsHide: true,
    env: { ...process.env, PROMPTIFY_TEST_WINDOW: title },
    timeout: 75000,
  });
}
