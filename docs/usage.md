# Usage guide

[← Back to README](../README.md) · [简体中文](usage.zh-CN.md)

## Host (default)

Double-click to run (on Windows install it first, see [Windows installation](#windows-installation)). A tray icon appears in the bottom-right corner (on macOS in the menu bar at the top of the screen, on Linux in the top bar / panel status area). Grey means not connected to the server, coloured means connected. The right-click menu (a plain click on macOS) contains:

- Current mode and server status
- The **control code** and "Copy control code"
- Who is controlling this computer right now, and **Disconnect all controllers**
- **Change control code**: the old code stops working immediately and connected controllers are disconnected
- Open console and **Start with Windows** (Windows) / Switch to controller (macOS, Linux) / Language / Quit

After a network drop the host reconnects automatically with exponential backoff and **gets its old control code back**, even after a server restart. The code is **fixed**: it's saved in `config.toml`, and the server keeps it for this computer however long it stays offline (codes unused for 180 days are released), so it stays the same across restarts and upgrades until you change it.

> On Linux the tray icon uses AppIndicator. It works out of the box on Ubuntu, KDE, Xfce and similar; stock GNOME needs the "AppIndicator and KStatusNotifierItem Support" extension. Without an AppIndicator library the program still runs as a host and prints the control code to the log (stderr).

> On macOS the host needs two system permissions: **Screen & System Audio Recording** (to capture the screen and sound) and **Accessibility** (to control the keyboard and mouse). macOS asks on the first start; while one is missing, the menu says so, and clicking that item opens the matching page in System Settings → Privacy & Security. A connected controller is told which permission is missing too (in the controller's own language), and the message goes away once it is granted. After granting Screen & System Audio Recording, reopen Airhand. Airhand normally lives in the menu bar only and shows up in the Dock while the console is open.

## Windows installation

On Windows Airhand is installed with its installer (into `C:\Program Files\Airhand` for all users; needs administrator rights). The host runs as the system service **Airhand**, as SYSTEM in the session of whoever is at the screen, so it keeps working on the lock screen, the sign-in screen and UAC prompts, and follows when users are switched. The tray icon is an ordinary program that shows the service's status and controls it:

- Opening Airhand starts the service; **Quit** in the tray stops it (as before, the computer can't be controlled after quitting); when the service stops (or is uninstalled) the tray icons close too
- **Start with Windows** (tray menu, on by default): the service starts at boot, so the computer can be controlled before anyone signs in, and the tray icon appears after signing in
- The configuration file `config.toml` is in the install directory (created on installation, see [Configuration file](#configuration-file)) and maintained by the service; edit it as administrator
- **Uninstalling** (Settings → Apps) stops and removes the service, the startup entry, `config.toml` and the program; nothing is left behind. Installing a new version over an old one keeps `config.toml`, and with it the control code and My devices, even if you choose to uninstall the old version first
- A bare `airhand.exe` doesn't run without being installed: release builds ask you to run the installer. Debug builds (`npm run tauri dev`) run the host inside the tray process, like on macOS and Linux, with `config.toml` next to the program (`client/src-tauri/target/debug/`)

## Configuration file

Everything Airhand saves is in one `config.toml` (with comments). The menu in the console's top-left corner also shows where it is:

| System | Location |
|---|---|
| Windows | The install directory: `C:\Program Files\Airhand\config.toml` (next to the program for debug builds that aren't installed) |
| macOS | `~/Library/Application Support/app.airhand.client/config.toml` |
| Linux | `$XDG_CONFIG_HOME/airhand/config.toml`, by default `~/.config/airhand/config.toml` |

| Key | Description |
|---|---|
| `server` | Relay server `host:port`; without it, the address built into the program |
| `server_fp` | Relay certificate fingerprint; `""` turns verification off (development only); without it, the built-in fingerprint |
| `autostart` | Start with Windows (Windows only, also in the tray menu) |
| `[code]` | This computer's control code, and the secret that gets it back. Delete `code` and `secret` to get a new code |
| `[[device]]` | The console's **My devices**: computers you've connected to (control code, their name, the name you gave them, last quality settings, last connected). Saved automatically on connect, up to 50. To add one by hand, write the 6 digits as `code` |

- Restart Airhand after changing settings (Quit, then open it again). Airhand keeps your edits and comments when it saves the code or devices
- **Deleted by accident** (the file or the whole directory): it's recreated with the default settings at startup, and within a few seconds while running. This computer's control code, the autostart setting and My devices carry over from before the deletion, so nothing is lost; other settings go back to their defaults
- A control code is enough to control its computer, so treat it like a key. On macOS / Linux only your account can read and write the file (mode 600, directory 700). On Windows the file is in the install directory, readable by every account on the computer: this computer's secret is encrypted with DPAPI for the SYSTEM account, and the codes in My devices with the Windows account that saved them, so other accounts and other computers can't read them (each account on the same computer only sees its own devices)
- Windows removes it when uninstalling; on macOS / Linux it stays after the program is removed (as is customary there), so delete the directory above if you don't need it anymore

## Controller

Tray → **Switch to controller** opens the console. Closing the console window switches back to host mode. On Windows the item is **Open console**: the host keeps running in the service while the console is open.

- It works like a browser: each computer gets a **tab** that you can drag to reorder and close (= disconnect) with × or a middle click. Click **+** for a new connection, enter the 6-digit control code (pasting `123 456` works), and once connected that tab becomes the computer. With two or more connections a **Multi-screen overview** tab appears on the left
- **My devices**: computers you connect to are saved (in `config.toml`); next time, one click on the new-connection page connects with the quality settings you used last. Rename or remove them (with undo), and search when there are many. If one is offline, there's no error: its tab opens and waits, and connects as soon as it's back (using the saved reconnect token, which doesn't count as code guessing)
- The window has no system title bar: drag an empty part of the tab bar to move it, double-click to maximize. The icon on the far left opens the menu: this computer's name, server status, language, switch to host. Closing the console while connections are open (including from the taskbar or Alt+F4) asks first
- Shortcuts (when not controlling a computer): Ctrl+T new connection, Ctrl+W close tab, Ctrl+Tab / Ctrl+Shift+Tab switch tabs, Ctrl+1–9, Ctrl+Shift+T reopen the last closed connection. While the remote screen has focus, every key goes to the remote computer
- Toolbar (like an address bar): remote name, control code, frame rate / bitrate / latency, encoder; monitor switch, **Quality**, shortcuts, clipboard (two-way sync / type as keystrokes), sound (mute / A/V sync), actual size (1:1), fullscreen, disconnect
- In fullscreen the tab bar is hidden and the toolbar slides in when the mouse touches the top edge; click the remote name to switch to another connection without leaving fullscreen. The console also tries to lock the keyboard, so system shortcuts such as the Win key and Alt+Tab go to the remote computer
- Network hiccups reconnect automatically. If the user at the host disconnects you, the console says so and doesn't reconnect
- If the remote computer goes offline (shut down, asleep, network gone), the console shows "Waiting for the remote computer to come back" and reconnects as soon as it's back with its old code. It doesn't keep retrying in the meantime, and the server doesn't mistake the reconnects for code guessing
- When connecting to a phone, its owner may have to accept first (the Android default): the console shows "Waiting for … to accept" and starts once accepted; if it's declined or ignored, the reason is shown and it doesn't retry. With a phone, the shortcut menu holds system actions such as Back, Home and notifications

**Quality panel**: presets from Data saver (720p / 24 fps / 1.2 Mbps) through Balanced (1080p / 30 fps / 4 Mbps), Smooth (1080p / 60 fps / 8 Mbps) and HD (1440p / 60 fps / 16 Mbps) to Original (native resolution / 60 fps / 30 Mbps), or set the resolution cap, frame rate (15–120), bitrate cap (0.5–50 Mbps), codec and adaptive bitrate yourself. The console only offers codecs this computer can hardware-decode; if the host has no matching hardware encoder it falls back automatically, and the toolbar shows the encoder actually in use.

## Language

The console and tray menu are available in **English** and **Simplified Chinese**. By default the client follows the system display language: Chinese if it is Chinese, English otherwise. Switch any time from the tray menu (Language / 语言) or the 中文 / EN switch in the console menu (the icon at the top-left). The choice isn't saved: the next start follows the system language again.

Notices and errors from the other computer or the server are sent as message types, not text, so they always appear in **your** language. Logs (client and server) are always in English.
