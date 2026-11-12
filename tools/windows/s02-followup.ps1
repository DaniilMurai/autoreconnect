<#
.SYNOPSIS
  S-02 follow-up for AutoReconnect: what a NON-admin process may do to Wi-Fi on this Windows laptop.

.DESCRIPTION
  Answers three questions and prints a short report to copy back:
    1. Can the Wi-Fi radio be switched off and on without admin?   (WlanSetInterface, and WinRT Radio as a second route)
    2. Does WlanConnect work on an ALL-USER profile?                (disconnect, then connect by profile name)
    3. Is the SSID readable with Location consent OFF?              (read-only; run the script once with Location ON, once OFF)

  SAFETY
    * Run it in a NORMAL (not "Run as administrator") PowerShell window: the point is what an ordinary user may do.
      An elevated window is refused unless -AllowElevated is given.
    * Total time offline is capped at 60 s for the whole run. A step that would not fit is skipped.
    * Whatever happens (error, Ctrl+C, timeout) the `finally` block turns the radio on again and reconnects the
      network that was connected at the start.
    * No profile is created, changed or deleted. No Wi-Fi password and no SSID text is read or printed.
    * Close video calls and downloads first: the connection drops for a few seconds, twice or three times.

  HOW TO RUN (twice)
    1. Settings -> Privacy & security -> Location: Location services ON.      Run:  .\s02-followup.ps1 -Label LocationOn
    2. Same page: Location services OFF (or "Let desktop apps access" OFF).   Run:  .\s02-followup.ps1 -Label LocationOff -ReadOnly
       (-ReadOnly skips the steps that drop the connection; question 3 needs no outage.)
    Send back the two report files named at the end of each run.

  Use "Windows PowerShell" (5.1), not PowerShell 7: the WinRT step only exists there (it is skipped with a note otherwise).
  Allow the script once if needed:  powershell -ExecutionPolicy Bypass -File .\s02-followup.ps1 -Label LocationOn
#>
[CmdletBinding()]
param(
    [string]$Label = 'run',
    [switch]$ReadOnly,
    [switch]$AllowElevated,
    [switch]$SkipWinRT
)

$ErrorActionPreference = 'Stop'
$MaxOfflineSec = 60
$MinStepBudgetSec = 20          # a step starts only if this much of the budget is left
$ReconnectWaitSec = 15
$RadioOffHoldSec = 3

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class W
{
    [DllImport("wlanapi.dll")] static extern uint WlanOpenHandle(uint ver, IntPtr res, out uint neg, out IntPtr h);
    [DllImport("wlanapi.dll")] static extern uint WlanCloseHandle(IntPtr h, IntPtr res);
    [DllImport("wlanapi.dll")] static extern uint WlanEnumInterfaces(IntPtr h, IntPtr res, out IntPtr list);
    [DllImport("wlanapi.dll")] static extern void WlanFreeMemory(IntPtr p);
    [DllImport("wlanapi.dll")] static extern uint WlanQueryInterface(IntPtr h, ref Guid g, int op, IntPtr res, out uint size, out IntPtr data, IntPtr type);
    [DllImport("wlanapi.dll")] static extern uint WlanSetInterface(IntPtr h, ref Guid g, int op, uint size, IntPtr data, IntPtr res);
    [DllImport("wlanapi.dll")] static extern uint WlanDisconnect(IntPtr h, ref Guid g, IntPtr res);
    [DllImport("wlanapi.dll")] static extern uint WlanConnect(IntPtr h, ref Guid g, ref Params p, IntPtr res);
    [DllImport("wlanapi.dll")] static extern uint WlanGetProfileList(IntPtr h, ref Guid g, IntPtr res, out IntPtr list);

    [StructLayout(LayoutKind.Sequential)]
    struct Params { public int Mode; public IntPtr Profile; public IntPtr Ssid; public IntPtr Bssids; public int BssType; public uint Flags; }

    const int OpRadio = 4, OpState = 6, OpCurrent = 7;

    public static IntPtr Open()
    {
        uint neg; IntPtr h;
        uint rc = WlanOpenHandle(2, IntPtr.Zero, out neg, out h);
        if (rc != 0) throw new Exception("WlanOpenHandle rc=" + rc);
        return h;
    }
    public static void Close(IntPtr h) { WlanCloseHandle(h, IntPtr.Zero); }

    public static Guid FirstInterface(IntPtr h)
    {
        IntPtr list;
        uint rc = WlanEnumInterfaces(h, IntPtr.Zero, out list);
        if (rc != 0) throw new Exception("WlanEnumInterfaces rc=" + rc);
        try
        {
            if (Marshal.ReadInt32(list, 0) < 1) throw new Exception("no Wi-Fi interface");
            byte[] b = new byte[16];
            Marshal.Copy(IntPtr.Add(list, 8), b, 0, 16);
            return new Guid(b);
        }
        finally { WlanFreeMemory(list); }
    }

    // 1 = connected, 0 = other state, -1 = query failed
    public static int Connected(IntPtr h, Guid g)
    {
        uint size; IntPtr data;
        if (WlanQueryInterface(h, ref g, OpState, IntPtr.Zero, out size, out data, IntPtr.Zero) != 0) return -1;
        try { return Marshal.ReadInt32(data) == 1 ? 1 : 0; } finally { WlanFreeMemory(data); }
    }

    // 1 = radio on, 0 = off (either switch), -1 = query failed
    public static int RadioOn(IntPtr h, Guid g)
    {
        uint size; IntPtr data;
        if (WlanQueryInterface(h, ref g, OpRadio, IntPtr.Zero, out size, out data, IntPtr.Zero) != 0) return -1;
        try
        {
            int n = Marshal.ReadInt32(data, 0);
            for (int i = 0; i < n; i++)
            {
                int sw = Marshal.ReadInt32(data, 4 + i * 12 + 4), hw = Marshal.ReadInt32(data, 4 + i * 12 + 8);
                if (sw == 1 && hw == 1) return 1;
            }
            return 0;
        }
        finally { WlanFreeMemory(data); }
    }

    // Software radio state of every PHY. Returns the first non-zero error code.
    public static uint SetRadio(IntPtr h, Guid g, bool on)
    {
        uint size; IntPtr data;
        uint rc = WlanQueryInterface(h, ref g, OpRadio, IntPtr.Zero, out size, out data, IntPtr.Zero);
        if (rc != 0) return rc;
        try
        {
            int n = Marshal.ReadInt32(data, 0);
            for (int i = 0; i < n; i++)
            {
                IntPtr req = Marshal.AllocHGlobal(12);
                try
                {
                    int state = on ? 1 : 2;
                    Marshal.WriteInt32(req, 0, Marshal.ReadInt32(data, 4 + i * 12));
                    Marshal.WriteInt32(req, 4, state);
                    Marshal.WriteInt32(req, 8, state);
                    rc = WlanSetInterface(h, ref g, OpRadio, 12, req, IntPtr.Zero);
                    if (rc != 0) return rc;
                }
                finally { Marshal.FreeHGlobal(req); }
            }
            return 0;
        }
        finally { WlanFreeMemory(data); }
    }

    // rc of the current-connection query (5 = access denied); profile name and SSID length are 0/"" when refused.
    public static uint Current(IntPtr h, Guid g, out string profile, out int ssidLen)
    {
        profile = ""; ssidLen = 0;
        uint size; IntPtr data;
        uint rc = WlanQueryInterface(h, ref g, OpCurrent, IntPtr.Zero, out size, out data, IntPtr.Zero);
        if (rc != 0) return rc;
        try
        {
            profile = Marshal.PtrToStringUni(IntPtr.Add(data, 8), 256).Split('\0')[0];
            ssidLen = Marshal.ReadInt32(data, 520);
            return 0;
        }
        finally { WlanFreeMemory(data); }
    }

    // WLAN_PROFILE_* flags of a named profile: 2 = per-user, 0 = all-user, -1 = not found / error.
    public static int ProfileFlags(IntPtr h, Guid g, string name)
    {
        IntPtr list;
        if (WlanGetProfileList(h, ref g, IntPtr.Zero, out list) != 0) return -1;
        try
        {
            int n = Marshal.ReadInt32(list, 0);
            for (int i = 0; i < n; i++)
            {
                IntPtr item = IntPtr.Add(list, 8 + i * 516);
                if (Marshal.PtrToStringUni(item, 256).Split('\0')[0] == name) return Marshal.ReadInt32(item, 512);
            }
            return -1;
        }
        finally { WlanFreeMemory(list); }
    }

    public static uint Disconnect(IntPtr h, Guid g) { return WlanDisconnect(h, ref g, IntPtr.Zero); }

    public static uint Connect(IntPtr h, Guid g, string profile)
    {
        Params p = new Params();
        p.Mode = 0;                       // wlan_connection_mode_profile
        p.BssType = 3;                    // dot11_BSS_type_any
        p.Profile = Marshal.StringToHGlobalUni(profile);
        try { return WlanConnect(h, ref g, ref p, IntPtr.Zero); }
        finally { Marshal.FreeHGlobal(p.Profile); }
    }
}
'@

# ---------------------------------------------------------------- report plumbing
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$reportPath = Join-Path $env:TEMP "autoreconnect-s02-$Label-$stamp.txt"
$report = New-Object System.Collections.Generic.List[string]
function Say([string]$line) {
    Write-Host $line
    $report.Add($line)
}
function Result([string]$key, [string]$value) { Say ("RESULT {0,-34} {1}" -f $key, $value) }

$script:handle = [IntPtr]::Zero
$script:guid = [Guid]::Empty
$script:profile = ''
$script:offline = [Diagnostics.Stopwatch]::new()   # accumulates only while we have cut the link
$script:radioTouched = $false
$script:winrtRadio = $null

function Budget-Left { [Math]::Max(0, $MaxOfflineSec - [int]$script:offline.Elapsed.TotalSeconds) }

# Waits are clipped to what is left of the offline budget, so the 60 s cap holds even when a step goes badly.
function Wait-Budget([int]$sec) { Wait-Connected ([Math]::Min($sec, [Math]::Max(1, (Budget-Left)))) }

function Wait-Connected([int]$sec) {
    $end = [DateTime]::UtcNow.AddSeconds($sec)
    while ([DateTime]::UtcNow -lt $end) {
        if ([W]::Connected($script:handle, $script:guid) -eq 1) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# Puts the laptop back the way it was; safe to call more than once.
function Restore {
    if ($script:handle -eq [IntPtr]::Zero) { return }
    try {
        if ($script:winrtRadio) {
            try { Await-Op ($script:winrtRadio.SetStateAsync([Windows.Devices.Radios.RadioState]::On)) | Out-Null } catch { }
        }
        if ([W]::RadioOn($script:handle, $script:guid) -ne 1) { [void][W]::SetRadio($script:handle, $script:guid, $true) }
        if ([W]::Connected($script:handle, $script:guid) -ne 1 -and $script:profile) {
            [void][W]::Connect($script:handle, $script:guid, $script:profile)
            [void](Wait-Connected 20)
        }
    } catch { Say "restore error: $($_.Exception.Message)" }
}

# ---------------------------------------------------------------- WinRT helpers (best effort)
function Await-Op($op) {
    $asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() |
        Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } |
        Select-Object -First 1
    $task = $asTask.MakeGenericMethod($op.GetType().GetInterfaces().Where({ $_.Name -eq 'IAsyncOperation`1' })[0].GenericTypeArguments[0]).Invoke($null, @($op))
    if (-not $task.Wait(15000)) { throw 'WinRT call timed out' }
    return $task.Result
}

# ---------------------------------------------------------------- run
try {
    Say "AutoReconnect S-02 follow-up   label=$Label   $(Get-Date -Format s)"
    $os = Get-CimInstance Win32_OperatingSystem
    Result 'os' "$($os.Caption) build $($os.BuildNumber)"
    Result 'powershell' $PSVersionTable.PSVersion.ToString()

    $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    Result 'elevated' $elevated
    if ($elevated -and -not $AllowElevated) {
        Say 'Refusing to run elevated: an admin window answers nothing about a normal user. Open a normal PowerShell and run again.'
        return
    }

    # Location state as Windows records it (informational; the real test is whether the SSID query works).
    foreach ($scope in 'HKLM', 'HKCU') {
        try {
            $v = (Get-ItemProperty "${scope}:\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\location" -ErrorAction Stop).Value
            Result "location_consent_$scope" $v
        } catch { Result "location_consent_$scope" 'unreadable' }
    }

    $script:handle = [W]::Open()
    $script:guid = [W]::FirstInterface($script:handle)
    $state = [W]::Connected($script:handle, $script:guid)
    Result 'connected_at_start' $state
    if ($state -ne 1) { Say 'Not connected to Wi-Fi at start: connect to your usual network and run again. Nothing was changed.'; return }

    # ---- Question 3: SSID readable? (read-only)
    $profName = ''; $ssidLen = 0
    $rc = [W]::Current($script:handle, $script:guid, [ref]$profName, [ref]$ssidLen)
    Result 'current_connection_query_rc' "$rc  (0 = ok, 5 = access denied)"
    Result 'ssid_readable' ($rc -eq 0 -and $ssidLen -gt 0)
    if ($rc -eq 0) {
        $script:profile = $profName
        Result 'ssid_length' $ssidLen
    } else {
        # Without the SSID the profile name is unknown; the profile list still names the connected network
        # through netsh only when Location allows it, so the outage steps need the query to work.
        Say 'Profile name unreadable (Location consent off): the outage steps below are skipped, they could not reconnect safely.'
    }

    if ($ReadOnly -or $rc -ne 0) {
        Say 'Read-only run: no radio or connection changes made.'
        return
    }

    $flags = [W]::ProfileFlags($script:handle, $script:guid, $script:profile)
    $allUser = ($flags -ge 0 -and ($flags -band 2) -eq 0)
    Result 'profile_flags' $flags
    Result 'profile_is_all_user' $allUser

    # ---- Question 2: WlanConnect on the (all-user) profile
    if ((Budget-Left) -ge $MinStepBudgetSec) {
        Say 'Step: disconnect, then WlanConnect by profile name ...'
        $script:offline.Start()
        try {
            $rcD = [W]::Disconnect($script:handle, $script:guid)
            Result 'wlan_disconnect_rc' $rcD
            Start-Sleep -Seconds 1
            $rcC = [W]::Connect($script:handle, $script:guid, $script:profile)
            Result 'wlan_connect_rc' "$rcC  (0 = ok, 5 = access denied)"
            $back = Wait-Budget $ReconnectWaitSec
            Result 'wlan_connect_reconnected' $back
        } finally {
            if ([W]::Connected($script:handle, $script:guid) -ne 1) { Restore }
            $script:offline.Stop()
        }
    } else { Say 'skipped (offline budget)' }

    # ---- Question 1a: radio off/on via WlanSetInterface
    if ((Budget-Left) -ge $MinStepBudgetSec) {
        Say "Step: radio off for $RadioOffHoldSec s via WlanSetInterface, then on ..."
        $script:offline.Start()
        $script:radioTouched = $true
        try {
            $rcOff = [W]::SetRadio($script:handle, $script:guid, $false)
            Result 'wlan_radio_off_rc' "$rcOff  (0 = ok, 5 = access denied)"
            if ($rcOff -eq 0) {
                Result 'radio_reads_off' ([W]::RadioOn($script:handle, $script:guid) -eq 0)
                Start-Sleep -Seconds $RadioOffHoldSec
            }
            $rcOn = [W]::SetRadio($script:handle, $script:guid, $true)
            Result 'wlan_radio_on_rc' $rcOn
            $back = Wait-Budget $ReconnectWaitSec
            Result 'wlan_radio_reconnected_by_os' $back
            if (-not $back) {
                [void][W]::Connect($script:handle, $script:guid, $script:profile)
                Result 'wlan_radio_reconnected_after_connect' (Wait-Budget $ReconnectWaitSec)
            }
        } finally {
            Restore
            $script:offline.Stop()
        }
    } else { Say 'skipped (offline budget)' }

    # ---- Question 1b: radio off/on via WinRT Windows.Devices.Radios (second route)
    if (-not $SkipWinRT -and (Budget-Left) -ge $MinStepBudgetSec) {
        Say 'Step: radio off/on via WinRT Radio.SetStateAsync ...'
        try {
            [void][Windows.Devices.Radios.Radio, Windows.System.Devices, ContentType = WindowsRuntime]
            [void][Windows.Devices.Radios.RadioAccessStatus, Windows.System.Devices, ContentType = WindowsRuntime]
            $access = Await-Op ([Windows.Devices.Radios.Radio]::RequestAccessAsync())
            Result 'winrt_request_access' $access
            $radios = Await-Op ([Windows.Devices.Radios.Radio]::GetRadiosAsync())
            $wifi = $radios | Where-Object { $_.Kind -eq 'WiFi' } | Select-Object -First 1
            if (-not $wifi) { Result 'winrt_wifi_radio_found' $false }
            else {
                Result 'winrt_wifi_radio_found' $true
                $script:winrtRadio = $wifi
                $script:offline.Start()
                try {
                    $offStatus = Await-Op ($wifi.SetStateAsync([Windows.Devices.Radios.RadioState]::Off))
                    Result 'winrt_radio_off_status' "$offStatus  (Allowed = ok)"
                    Start-Sleep -Seconds $RadioOffHoldSec
                    $onStatus = Await-Op ($wifi.SetStateAsync([Windows.Devices.Radios.RadioState]::On))
                    Result 'winrt_radio_on_status' $onStatus
                    Result 'winrt_reconnected_by_os' (Wait-Budget $ReconnectWaitSec)
                } finally {
                    Restore
                    $script:offline.Stop()
                }
            }
        } catch {
            Result 'winrt_error' $_.Exception.Message
        }
    } else { Say 'WinRT step skipped' }
}
catch {
    Say "ERROR: $($_.Exception.Message)"
}
finally {
    Restore
    if ($script:handle -ne [IntPtr]::Zero) {
        Result 'connected_at_end' ([W]::Connected($script:handle, $script:guid))
        Result 'radio_on_at_end' ([W]::RadioOn($script:handle, $script:guid))
        [W]::Close($script:handle)
    }
    Result 'offline_seconds_total' ([Math]::Round($script:offline.Elapsed.TotalSeconds, 1))
    try {
        $report | Set-Content -Path $reportPath -Encoding UTF8
        Write-Host ''
        Write-Host "Report saved: $reportPath"
        Write-Host 'Send that file back (it holds no SSID and no password).'
    } catch { Write-Host "Could not save the report: $($_.Exception.Message)" }
}
