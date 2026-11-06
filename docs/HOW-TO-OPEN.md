# How to open AutoReconnect

AutoReconnect sits in the menu bar (Mac) or the tray (Windows) and reconnects Wi-Fi faster than the
system does. The builds are not signed by Apple or Microsoft, so the system warns you once. This is
expected; the steps below get past it.

## Mac

1. Double-click `AutoReconnect-<version>.zip` to unzip it, then drag `AutoReconnect.app` into
   **Applications**.
2. Open it. macOS says it cannot verify the app: click **Done** (not "Move to Trash").
3. Open **System Settings → Privacy & Security**, scroll down to the message about AutoReconnect and
   click **Open Anyway**. Enter your Mac password. The button is there for about an hour after the
   blocked launch; if it is gone, open the app once more.
4. A **Location** question appears: click **Allow**. macOS only shows a Wi-Fi network's name to
   apps that have Location access; the app needs the name to rejoin the right network.
5. On the first run the app asks for a password once to read your Wi-Fi password. This can be your Mac
   admin name and password, or your login (keychain) password. Enter it and click
   **Always Allow**: this means "do not ask again for this app", so it never asks again.
6. The app has no window. Look for its icon in the menu bar at the top right.

After each new version the Mac asks the Location question and the keychain password again, once.

## Windows

1. Unzip `AutoReconnect-<version>-windows.zip` and put `autoreconnect.exe` in a folder you keep
   (for example `C:\Tools\AutoReconnect`).
2. Double-click it. A blue "Windows protected your PC" window appears: click **More info**, then
   **Run anyway**.
3. If Windows blocks it with no "Run anyway" button, **Smart App Control** is on. It cannot allow a
   single app. Check: **Settings → Privacy & security → Windows Security → App & browser control →
   Smart App Control settings**. If it says **On**, the app cannot run unless you switch it **Off**.
   Off is permanent: Windows does not let you turn it on again without a reset. Decide before you do it.

## Quit

- Mac: click the menu bar icon, then **Quit**.
- Windows: click the tray icon (maybe under the `^` arrow), then **Quit**.

## If something is wrong: the log

Every action is written to `events.jsonl`, one line per event. Send this file when you report a problem.

- Mac: `~/Library/Application Support/AutoReconnect/events.jsonl`
  (Finder → Go → Go to Folder, paste `~/Library/Application Support/AutoReconnect`).
- Windows: `%LOCALAPPDATA%\AutoReconnect\events.jsonl` (paste it into the Explorer address bar).
