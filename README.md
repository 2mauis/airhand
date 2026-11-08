<p align="center"><img src="assets/icon.svg" width="96" alt="Airhand"></p>

<h1 align="center">Airhand</h1>
<p align="center">High-performance remote desktop over QUIC · Rust + Tauri + React · Android</p>
<p align="center"><b>English</b> · <a href="README.zh-CN.md">简体中文</a></p>

---

Airhand is a single program that is both the **host** (the computer being controlled) and the **controller** (the computer you control it from). It runs on **Windows**, **macOS** (13 or later) and **Linux** (X11 desktops). A small relay server, deployed with one Docker Compose command, handles signaling and forwards traffic. The controller can also be deployed with Docker Compose as a **web console** that you use from a browser.

On Windows Airhand is installed, and the host runs as a system service: the lock screen, the sign-in screen and UAC prompts can be controlled too, and the control code stays the same.

There is also an **Android** app (a single APK): on **phones and tablets** it can watch and control other devices, and let others watch and control the phone; opened from a **TV**'s home screen it shows a dedicated TV interface that only watches (see [Android](#android-phones--tvs)).

> **Status:** under active development. The protocol still changes between versions, so the clients and the server must be built from the same version (a mismatch is detected and reported, see [Protocol versions](#protocol-versions)).

## Features

- **Host by default**: after launch there is only a tray icon, no window, and no WebView is started
- **Windows system service**: the lock screen, the sign-in screen and UAC prompts can be seen and controlled; the **control code is fixed**; optionally starts with Windows (see [Windows installation](#windows-installation))
- **Controller console**: control **several computers at once**, switch between them, or watch them all in a multi-screen overview
- **Web console**: start the controller with Docker Compose and open one port in a browser; nothing to install on the computer you control from (see [Web console](#web-console-docker))
- **Android phones and TVs**: control computers and phones from a phone with trackpad or touch gestures, or share the phone's own screen to get help; on a TV just watch, with the remote. Devices you connected to are saved, so next time it's one tap (see [Android](#android-phones--tvs))
- **Low latency, low bandwidth**: hardware **AV1 / HEVC / H.264** encoding, no frames when the screen is still, adaptive bitrate, the cursor moves locally with no lag
- **Audio**: the host's system sound is streamed to the controller, with optional A/V sync for watching video
- **Practical tools**: two-way clipboard sync, typing the clipboard as keystrokes, system shortcuts (Win, Alt+Tab, Ctrl+Shift+Esc…), multiple monitors, fullscreen with keyboard lock
- **Leaves nothing behind**: on macOS and Linux there's no config file and nothing is written to disk; on Windows the only file is `config.toml` in the install directory, removed together with the service when uninstalling (see [below](#files-on-disk))
- **Secure by default**: TLS 1.3 with certificate pinning, brute-force protection for control codes
- **English and Simplified Chinese**: follows the system language, switchable at any time

## Contents

- [Quick start](#quick-start)
- [Usage](#usage)
- [Server configuration](#server-configuration)
- [Web console (Docker)](#web-console-docker)
- [Android (phones / TVs)](#android-phones--tvs)
- [Building the client](#building-the-client)
- [How it works](#how-it-works)
- [Security](#security)
- [Known limitations](#known-limitations)
- [Development](#development)

## Quick start

1. **Deploy the server** on a machine both computers can reach (only **UDP 4433** needs to be open):

   ```bash
   docker compose up -d
   docker compose logs airhand | grep fingerprint   # note the certificate SHA-256 fingerprint
   ```

2. **Build the client** with the server address and fingerprint compiled in (requirements and the macOS / Linux steps in [Building the client](#building-the-client)):

   ```powershell
   cd client
   npm install
   $env:AIRHAND_SERVER    = "relay.example.com:4433"
   $env:AIRHAND_SERVER_FP = "<fingerprint from step 1>"
   npm run tauri build
   # → client/src-tauri/target/release/bundle/nsis/Airhand_<version>_x64-setup.exe (installer)
   ```

   On Windows the address and fingerprint can also be changed after installing, in `config.toml` in the install directory (see [Windows installation](#windows-installation)).

3. **Install it on both computers** (on Windows run the installer, which needs administrator rights). On the remote one, right-click the tray icon to see its 6-digit **control code**. On yours, choose **Open console** (**Switch to controller** on macOS / Linux) in the tray menu and enter the code.

   If you'd rather not install the client on the computer you control from, use the [web console](#web-console-docker) instead: `cd web && AIRHAND_SERVER=relay.example.com:4433 AIRHAND_SERVER_FP=<fingerprint> docker compose up -d`, then open it in a browser.

## Usage

### Host (default)

Double-click to run (on Windows install it first, see [Windows installation](#windows-installation)). A tray icon appears in the bottom-right corner (on macOS in the menu bar at the top of the screen, on Linux in the top bar / panel status area). Grey means not connected to the server, coloured means connected. The right-click menu (a plain click on macOS) contains:

- Current mode and server status
- The **control code** and "Copy control code"
- Who is controlling this computer right now, and **Disconnect all controllers**
- **Change control code**: the old code stops working immediately and connected controllers are disconnected
- Open console and **Start with Windows** (Windows) / Switch to controller (macOS, Linux) / Language / Quit

After a network drop the host reconnects automatically with exponential backoff and **gets its old control code back**, even after a server restart. On Windows the code is **fixed**: it's saved in `config.toml`, and the server keeps it for this computer however long it stays offline (codes unused for 180 days are released), so it stays the same across restarts and upgrades until you change it. On macOS and Linux each start gets a new code, and the server keeps a code for its host for 10 minutes after the host goes offline.

> On Linux the tray icon uses AppIndicator. It works out of the box on Ubuntu, KDE, Xfce and similar; stock GNOME needs the "AppIndicator and KStatusNotifierItem Support" extension. Without an AppIndicator library the program still runs as a host and prints the control code to the log (stderr).

> On macOS the host needs two system permissions: **Screen & System Audio Recording** (to capture the screen and sound) and **Accessibility** (to control the keyboard and mouse). macOS asks on the first start; while one is missing, the menu says so, and clicking that item opens the matching page in System Settings → Privacy & Security. A connected controller is told which permission is missing too (in the controller's own language), and the message goes away once it is granted. After granting Screen & System Audio Recording, reopen Airhand. Airhand normally lives in the menu bar only and shows up in the Dock while the console is open.

### Windows installation

On Windows Airhand is installed with its installer (into `C:\Program Files\Airhand` for all users; needs administrator rights). The host runs as the system service **Airhand**, as SYSTEM in the session of whoever is at the screen, so it keeps working on the lock screen, the sign-in screen and UAC prompts, and follows when users are switched. The tray icon is an ordinary program that shows the service's status and controls it:

- Opening Airhand starts the service; **Quit** in the tray stops it (as before, the computer can't be controlled after quitting); when the service stops (or is uninstalled) the tray icons close too
- **Start with Windows** (tray menu, on by default): the service starts at boot, so the computer can be controlled before anyone signs in, and the tray icon appears after signing in
- `config.toml` in the install directory (created on installation, with comments):

  | Key | Description |
  |---|---|
  | `server` | Relay server `host:port`; without it, the address built into the program |
  | `server_fp` | Relay certificate fingerprint; `""` turns verification off (development only); without it, the built-in fingerprint |
  | `autostart` | Start with Windows (also in the tray menu) |
  | `[code]` | The control code the server assigned, and the secret that gets it back (encrypted with DPAPI, only Airhand on this computer can read it). Delete `code` and `secret` to get a new code |

  Edit it as administrator, then restart Airhand (Quit, then open it again). Airhand keeps your edits and comments when it saves the code or the autostart setting
- **Uninstalling** (Settings → Apps) stops and removes the service, the startup entry, `config.toml` and the program; nothing is left behind. Installing a new version over an old one keeps `config.toml`, and with it the control code, even if you choose to uninstall the old version first
- A bare `airhand.exe` doesn't run without being installed: release builds ask you to run the installer. Debug builds (`npm run tauri dev`) run the host inside the tray process, like on macOS and Linux, with `config.toml` next to the program (`client/src-tauri/target/debug/`)

### Controller

Tray → **Switch to controller** opens the console. Closing the console window switches back to host mode. On Windows the item is **Open console**: the host keeps running in the service while the console is open.

- Enter a 6-digit control code to connect (pasting `123 456` works). Connect to as many computers as you like, switch between them in the left-hand list, or open the **Multi-screen overview**
- Toolbar: monitor switch, **Quality**, shortcuts, clipboard (two-way sync / type as keystrokes), sound (mute / A/V sync), actual size (1:1), fullscreen, disconnect
- In fullscreen the console tries to lock the keyboard, so system shortcuts such as the Win key and Alt+Tab go to the remote computer
- Network hiccups reconnect automatically. If the user at the host disconnects you, the console says so and doesn't reconnect
- If the remote computer goes offline (shut down, asleep, network gone), the console shows "Waiting for the remote computer to come back" and reconnects as soon as it's back with its old code. It doesn't keep retrying in the meantime, and the server doesn't mistake the reconnects for code guessing
- When connecting to a phone, its owner may have to accept first (the Android default): the console shows "Waiting for … to accept" and starts once accepted; if it's declined or ignored, the reason is shown and it doesn't retry. With a phone, the shortcut menu holds system actions such as Back, Home and notifications

**Quality panel**: presets from Data saver (720p / 24 fps / 1.2 Mbps) through Balanced (1080p / 30 fps / 4 Mbps), Smooth (1080p / 60 fps / 8 Mbps) and HD (1440p / 60 fps / 16 Mbps) to Original (native resolution / 60 fps / 30 Mbps), or set the resolution cap, frame rate (15–120), bitrate cap (0.5–50 Mbps), codec and adaptive bitrate yourself. The console only offers codecs this computer can hardware-decode; if the host has no matching hardware encoder it falls back automatically, and the toolbar shows the encoder actually in use.

### Language

The console and tray menu are available in **English** and **Simplified Chinese**. By default the client follows the system display language: Chinese if it is Chinese, English otherwise. Switch any time from the tray menu (Language / 语言) or the 中文 / EN switch at the bottom-left of the console; like every other setting it isn't saved.

Notices and errors from the other computer or the server are sent as message types, not text, so they always appear in **your** language. Logs (client and server) are always in English.

## Server configuration

The server listens on **UDP 4433**. On first start it generates a self-signed certificate in the `data/` directory next to `docker-compose.yml`. **Keep that directory**: clients recognise the server by the certificate's fingerprint, and it holds the SQLite database with the fixed control codes of Windows hosts (`airhand.db`, plus its `-wal` / `-shm` files while the server runs).

Environment variables (see `docker-compose.yml`):

| Variable | Default | Description |
|---|---|---|
| `AIRHAND_LISTEN` | `0.0.0.0:4433` | Listen address (UDP) |
| `AIRHAND_CERT` / `AIRHAND_KEY` | `/data/cert.pem` / `/data/key.pem` | Certificate paths; generated automatically if missing |
| `AIRHAND_CERT_NAMES` | `airhand,localhost` | SANs written into an auto-generated certificate |
| `AIRHAND_DB` | `airhand.db` next to the certificate | SQLite database with the fixed control codes (code, hash of the host's secret, last seen). Without it, codes of hosts that are offline could go to other hosts (a host still gets its code back if nobody took it). If it can't be opened the server keeps the codes in memory and logs an error |
| `AIRHAND_MAX_SESSIONS` | `4` | Max simultaneous controllers per host |
| `RUST_LOG` | `info` | Log level |

The image is based on distroless, runs as a non-root user, and is about 28 MB.

## Web console (Docker)

The controller can also run in a container: start it with Docker Compose and open one port in a browser to get the console, with nothing to install or build on the computer you control from. It's the same console as the desktop one (several sessions, multi-screen overview, quality, sound, clipboard…); the computers being controlled still run the Airhand client.

```bash
cd web
AIRHAND_SERVER=relay.example.com:4433 AIRHAND_SERVER_FP=<relay certificate fingerprint> docker compose up -d
# or: cp .env.example .env, edit it, then just docker compose up -d
```

Then open it in a browser:

- On the computer running the container: `http://localhost:8080`
- From other computers: `https://<this computer's address>:8080`

The same port speaks both HTTP and HTTPS. Browsers only offer WebCodecs video decoding on secure pages (HTTPS or localhost), so use https:// from other computers (opened over http://, the page links you to the https address). The certificate is a self-signed one generated on first start and kept in a data volume; the first time, the browser warns about it, and you continue to the site. Needs Chrome / Edge, Safari 16.4+ or Firefox 130+.

Environment variables (see `web/docker-compose.yml` and `web/.env.example`):

| Variable | Default | Description |
|---|---|---|
| `AIRHAND_SERVER` | (required) | Relay server address `host:port` |
| `AIRHAND_SERVER_FP` | empty | Relay certificate fingerprint. Empty means the certificate is **not verified**; for development only (the console shows "Not pinned") |
| `AIRHAND_NAME` | `Airhand Web` | The name shown in the host's tray under who is controlling it |
| `AIRHAND_WEB_PASSWORD` | empty | Access password: the browser shows a login prompt, any user name works. Empty means anyone who can reach the port can use the console (connecting to a computer still needs its control code) |
| `AIRHAND_WEB_PORT` | `8080` | Port published on the Docker host |
| `AIRHAND_WEB_CERT_NAMES` | `localhost` | Names / IPs written into the generated HTTPS certificate (comma-separated). List the address you actually use if you want the OS to trust the certificate |
| `AIRHAND_WEB_CERT` / `AIRHAND_WEB_KEY` | `/data/cert.pem` / `/data/key.pem` | HTTPS certificate paths; mount your own certificate here to use it |
| `RUST_LOG` | `info` | Log level |

- Each browser tab is a separate console with its own connection to the relay; closing or reloading the tab disconnects its sessions (the browser asks first while there are sessions). If the connection to the web service drops, the page reconnects and the sessions resume on their own
- Frames are acknowledged only once the browser has them, so the host's in-flight frame control covers the service-to-browser link too, and a slow link there doesn't make frames pile up. The latency in the toolbar is the service-host round trip plus the browser-service round trip, so run the service close to the browser (on the same LAN, for example)
- The clipboard is the browser's: text copied on the remote computer is written to this computer's clipboard (while the page is in front); text copied here is sent when you come back to the page, which needs the browser's permission to read the clipboard (Chrome / Edge ask the first time). Firefox doesn't let pages read the clipboard on their own, so there it only syncs from the remote side
- Outside fullscreen the browser keeps some shortcuts (Ctrl+W, Ctrl+T…) for itself; in fullscreen Chrome / Edge lock the keyboard, so the Win key, Alt+Tab and the like go to the remote computer
- Behind a reverse proxy (for your own domain and certificate): proxy to the container's `http://…:8080`, forward WebSockets (`/ws`) and keep the `Host` header (or set `X-Forwarded-Host`), otherwise the WebSocket is rejected as cross-site. A sub-path works too
- The image is based on distroless, runs as a non-root user, and is about 30 MB. It only needs outbound UDP (to the relay) and this one inbound TCP port

## Android (phones / TVs)

One APK (`android/`, see [building](#android-apk)) for phones, tablets and TVs. The same app shows a different interface depending on how it's opened:

| Opened from | Interface | What it does |
|---|---|---|
| A phone / tablet home screen | Phone interface (three tabs) | **Control others**: watch and operate computers or other phones<br>**Be controlled**: share this phone's screen so others can watch and operate it |
| A TV home screen (Android TV / Google TV / TV boxes) | TV interface, everything works with the remote | **Watch only** (computers or phones); no control, can't be controlled |

On a TV, the phone entry point also switches to the TV interface.

### Phone: Remote

- The home page is a **6-digit control code field**; it connects as soon as 6 digits are in ("Paste code" also finds a code inside text like "code: 123 456"). Connecting happens right there (a spinner on the button) and the remote screen opens once connected; a wrong code, an offline device, too many attempts and so on are explained right under the field
- **My devices**: every device you connect to is saved (name, OS, control code, last connection, resume token), so next time it's **one tap**. If it's offline the row says "Offline · connects automatically when it's back" and the remote screen opens by itself when it comes online; you can cancel any time. Devices can be renamed, their code copied, or removed (with undo)
- A card at the top leads back to a session that's still running

The remote screen is full screen with just one **draggable round button** (it snaps to the left / right edge) that opens the menu: keyboard, shortcuts, sound, quality (same presets as the desktop, plus the video format), clipboard (send to the remote clipboard / type it in), displays (with several monitors), fit screen, gestures, disconnect. The top of the menu shows the device's name, video format, frame rate, latency and bitrate. Rotating the phone to landscape fills the screen.

Two **touch input modes** (two big buttons in the menu switch any time, Settings sets the default; the default "Automatic" uses the trackpad for computers and touch for phones). The gestures are explained the first time each mode is used:

| Gesture | Trackpad (computers) | Touch (computers) | Touch (phones) |
|---|---|---|---|
| Slide one finger | Move the pointer (precise when slow, accelerated when fast) | Drag with the left button | Every finger is passed through: taps, long presses, swipes, pinch-to-zoom and multi-finger gestures all happen on the remote phone |
| Tap | Click; tap twice = double-click | Click under the finger | 〃 |
| Long press | Right-click | Right-click under the finger | 〃 |
| Tap, then hold and drag | Drag with the left button (select text, move windows) | — | 〃 |
| Two-finger tap | Right-click | Right-click | 〃 |
| Two-finger drag | Scroll (natural, with momentum) | Scroll | 〃 |
| Pinch | Zoom in, the view follows the pointer | Zoom / move the view | 〃 |
| Three-finger tap | Show / hide the keyboard | Same | 〃 (the keyboard is in the menu) |

When the remote phone runs Android 7 (no continuous multi-finger gestures), only one finger is passed through and pinching zooms the local view.

- In trackpad mode the pointer is drawn locally (with the remote shape), so it moves without lag; in touch mode a fading circle shows where you tapped
- **Keyboard**: the picture shrinks to stay fully visible above it, and a row of extra keys sits on top: Esc, Tab, Ctrl, Alt, Shift, Win (⌘ for Macs), arrows, Home / End, PgUp / PgDn, Del, F1–F12. Modifiers are sticky: tap once for the next key only, again to lock, again to release (tap Ctrl, then type c = Ctrl+C). While an IME composes (pinyin etc.) the text shows above the extra keys and is only sent once chosen
- When controlling a phone, **Back / Home / Recents** keys sit below the picture (can be turned off in Settings), and the shortcut menu holds system actions: notifications, quick settings, screenshot, lock screen, power menu, volume (whichever the phone supports). For computers the shortcut menu lists shortcuts for the remote OS (Windows / macOS / Linux)
- If the remote phone has "Ask before someone connects" on, the home page shows "Waiting for … to accept" (cancellable) and the remote screen opens once it's accepted; if it's declined or ignored for 60 s, the reason is shown
- A connected mouse and keyboard map directly to the remote mouse and keyboard
- Back first asks "Disconnect?", so it can't happen by accident. Network drops reconnect automatically; if the device goes offline it says so and reconnects when it's back; if the other side disconnected you or changed its code, it says why and doesn't retry
- In the background the session stays, at 2 fps and without sound, and resumes immediately when you return

### Phone: Share screen (be controlled)

On the "Share screen" tab, tap **Start sharing screen**: Airhand asks for notification and audio-recording permission (recording is only used to share **what the phone is playing**, never the microphone; the screen is shared even if you decline), then the system asks to confirm screen recording. Once sharing:

- The **control code** is shown in large digits, with copy and share. Like the Windows installation it is **fixed** (saved in the app's own data directory, and the server keeps it for this phone) until you tap "Change control code"
- **Ask before someone connects** (on by default, can be turned off in Settings): when someone enters this phone's code, a notification pops up and this tab shows "… wants to connect", saying whether they'll be able to watch or control and from which IP; you **Allow** or **Decline**, and it's declined automatically after 60 s. Until then they see nothing and can't do anything. A device you allowed reconnects without asking until sharing stops; after "Disconnect all" or a code change it has to be allowed again
- **Who's watching** (name, IP) is listed, with "Disconnect all"; the notification always shows "Sharing · control code" and "Stop sharing"
- The screen is kept on while someone is watching (nothing can be captured when it's off); with nobody watching nothing is captured or encoded
- Below is a **permission checklist**, each item with its purpose, state and a "Turn on" button:
  - **Remote control** (accessibility service): needed to tap, swipe and type; without it the other side can only watch (and is told so). From Android 13, apps not installed from a store must first be allowed in "App info → ⋮ at the top right → Allow restricted settings"; the guide explains it and links there
  - **Share sound** (Android 10+, audio-recording permission)
  - **Notifications** (Android 13+)
  - **Run in background** (ignore battery optimization): keeps the system from stopping sharing when locked or in the background

Remote input maps to the phone like this: touch points and the left mouse button are fingers (tap, long press, drag, swipe; several at once is multi-touch), the wheel swipes; Back, Home, Recents, notifications, screenshot and so on are **system actions** in the protocol, sent by the controller's navigation keys and shortcut menu (the desktop console's shortcut menu lists them too when the remote side is a phone). For people at a computer, right-click / Esc = Back, middle-click / Win alone = Home, Alt+Tab = Recents, Ctrl+C / V / X / A = copy / paste / cut / select all; typing goes straight into the focused text field (physical keys from desktop consoles are translated with a US layout).

### Phone: Settings

Server address and certificate fingerprint (defaults are compiled in and can be changed here; a malformed fingerprint can't be saved), this device's name (shown to others, defaults to the system device name), ask before someone connects, change control code, default quality, default touch input, play sound by default, navigation keys for phones, clear saved devices, version and protocol version.

### TV

- Enter the control code on the left with the remote's number keys or the on-screen keypad; recent devices are on the right (arrow keys to pick, OK to connect); Settings (server, fingerprint, device name) is at the top right
- The picture is full screen with sound on. **OK / Menu** opens the side menu: sound, displays, quality (each press picks the next preset), connection info, disconnect; **left / right** switch the remote display; **Back** closes the menu, and asks before disconnecting

### Where the data is stored

Settings (`settings.json`), saved devices (`devices.json`) and this phone's control code with the secret to reclaim it (`host.json`) are only kept in the app's private directory (`/data/data/app.airhand.android/files/`), which other apps can't read. Backup and device transfer are disabled in the manifest, and **uninstalling the app deletes all of it**. Logs only go to logcat.

## Building the client

Requirements: Rust (stable), Node.js 20+, CMake (to build libopus).

- **Windows**: Windows 10/11 (WebView2 ships with the OS)
- **macOS**: macOS 13 or later and the Xcode Command Line Tools (`xcode-select --install`); CMake can come from `brew install cmake`
- **Linux**: Tauri's dependencies, the PulseAudio development files and the GStreamer plugins the console uses to decode video. On Debian / Ubuntu:

  ```bash
  sudo apt install build-essential cmake pkg-config libssl-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev libxdo-dev libpulse-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
  ```

The server address and certificate fingerprint are **compiled into the program**: set `AIRHAND_SERVER` and `AIRHAND_SERVER_FP` at build time (as in [Quick start](#quick-start)), or change the defaults in `client/src-tauri/src/config.rs`. Without `AIRHAND_SERVER` the client connects to `127.0.0.1:4433`. A malformed fingerprint (it must be 64 hex digits; `:` and spaces are allowed) fails the build. On Windows these are only defaults: `server` and `server_fp` in `config.toml` override them (an invalid `server_fp` there refuses every connection rather than turning verification off).

> When `AIRHAND_SERVER_FP` is empty the **server certificate is not verified**, which leaves you open to man-in-the-middle attacks. Use that for development only; the console then shows "Not pinned" at the top left.

**Windows**: `npm run tauri build` produces the NSIS installer `client/src-tauri/target/release/bundle/nsis/Airhand_<version>_x64-setup.exe`, which installs for all users and registers the service (see [Windows installation](#windows-installation); the installer steps are in `client/src-tauri/windows/hooks.nsh`). `-- --no-bundle` only builds `airhand.exe`, which as a release build refuses to run until installed.

- The C runtime is statically linked, so the VC++ redistributable isn't needed
- Media Foundation is delay-loaded, so Windows N editions (without the Media Feature Pack) still run and use software encoding

**Linux** (set the variables with `export AIRHAND_SERVER=...`):

```bash
cd client && npm install
npm run tauri build
# → client/src-tauri/target/release/airhand (single file, ~10 MB)
#   client/src-tauri/target/release/bundle/deb/Airhand_<version>_amd64.deb (pulls in the tray, audio and GStreamer dependencies)
```

**macOS** (set the variables with `export` as well):

```bash
cd client && npm install
npm run tauri build
# → client/src-tauri/target/release/bundle/macos/Airhand.app (~8 MB)
#   client/src-tauri/target/release/bundle/dmg/Airhand_<version>_<arch>.dmg
```

- For just the .app use `npm run tauri build -- --bundles app`; it is built for the machine's own architecture (arm64 on Apple silicon)
- The app is only ad-hoc signed, not notarized: the first time you open a copy on another Mac, right-click it in Finder → Open (or run `xattr -dr com.apple.quarantine Airhand.app`)
- macOS remembers granted permissions by code signature, and an ad-hoc signature changes with every build: after rebuilding, remove Airhand in System Settings → Privacy & Security and grant the permissions again. For long-term use, put your own Developer ID in `signingIdentity` in `client/src-tauri/tauri.macos.conf.json`

### Android (APK)

Requirements: JDK 17 or later (the one bundled with Android Studio works), the Android SDK (platform 35) and NDK 27, Rust's Android targets and [cargo-ndk](https://github.com/bbqsrc/cargo-ndk):

```bash
# sdkmanager from the Android SDK (or tick them in Android Studio's SDK Manager)
sdkmanager "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
```

Then build in `android/` (Gradle first builds the Rust core in `android/native` with cargo-ndk):

```powershell
cd android
# tell Gradle where the Android SDK is (or set ANDROID_HOME)
"sdk.dir=C:/Users/<you>/AppData/Local/Android/Sdk" | Out-File -Encoding ascii local.properties
$env:AIRHAND_SERVER    = "relay.example.com:4433"   # compiled-in defaults, like the desktop; changeable in Settings later
$env:AIRHAND_SERVER_FP = "<server certificate fingerprint>"
./gradlew assembleRelease
# → android/app/build/outputs/apk/release/Airhand-<version>-release.apk (~12 MB, arm64-v8a / armeabi-v7a / x86_64)
```

- One APK for phones and TVs (Android 7.0 or later); `-Pairhand.abis=arm64-v8a` builds just one CPU type, faster and smaller
- **Signing**: with `AIRHAND_KEYSTORE` (plus `AIRHAND_KEYSTORE_PASSWORD`, `AIRHAND_KEY_ALIAS`, `AIRHAND_KEY_PASSWORD`) set, the APK is signed with your key; otherwise with this machine's debug key, which still installs fine. For APKs you hand out and update over time, always use the same key of your own
- `./gradlew assembleDebug` builds a debug version (package `app.airhand.android.debug`, installable next to the release)
- A malformed fingerprint fails the build (same rule as the desktop)

## How it works

### Architecture

```
 Host (Windows; others: see table)       airhand-server (Docker)                  Controller (Tauri / web)
┌───────────────────────────┐            ┌───────────────────────────┐            ┌───────────────────────────┐
│ DXGI capture (GPU)        │            │ Control-code registry     │            │ WebCodecs HW decode       │
│ SIMD scale + NV12 convert │  QUIC/TLS  │ Session pairing           │  QUIC/TLS  │ Low-latency Canvas        │
│ HW encode AV1/HEVC/H.264  │◀══════════▶│ Zero-copy byte relay      │◀══════════▶│ Input capture + cursor    │
│ SendInput injection       │   UDP 4433 │ Brute-force rate limiting │   UDP 4433 │ Multi-session / overview  │
│ WASAPI loopback + Opus    │            │                           │            │ Jitter-buffered audio     │
└───────────────────────────┘            └───────────────────────────┘            └───────────────────────────┘
```

The first bidirectional stream on every QUIC connection is the **control stream** (signaling). For each remote-control session both sides open another bidirectional stream; the server pairs them by session ID and from then on **only forwards bytes, it never parses the video**. Every message is a `u32` length prefix followed by a postcard-encoded payload. When a controller reconnects it presents the resume token it got when it connected; if the host is offline the server remembers the request and sends `HostOnline` when the host is back (see "Resume tokens" in `crates/airhand-proto/src/signal.rs`).

On the session stream the controller first sends a random ID for this run of the app; the host answers with `Hello` (platform, whether it accepts multi-touch, which system actions it supports) and starts sending video. A host may first answer "awaiting approval" and send `Hello` only after the person at it allows the connection (Android does by default; see the module docs of `crates/airhand-proto/src/session.rs`).

The web console splits the controller on the right in two: the QUIC connection and session handling run in `airhand-web` (the container), while decoding, rendering and input capture stay in the browser, with one WebSocket in between. Packets are forwarded to the browser unchanged (with a 4-byte session number in front), and the browser acknowledges the frames.

| Directory | Contents |
|---|---|
| `crates/airhand-proto` | Protocol: signaling, session messages, framing |
| `crates/airhand-link` | The client side of the relay link: QUIC, certificate pinning, reconnects, UI-independent controller sessions, `Problem`. Shared by the desktop client and the web console |
| `server/` | Relay server (quinn + rustls) and its `Dockerfile` |
| `web/` | Web console service (axum: static files + WebSocket, HTTP and HTTPS on one port), its `Dockerfile` and `docker-compose.yml`. A separate Cargo workspace, like `client/src-tauri/` |
| `client/src-tauri/` | Desktop client, Rust side: host `host/`, tray, console window and commands |
| `android/native/` | Android Rust core (JNI): the controller uses `airhand-link`; the host's network side (registering / reclaiming the code, sessions, in-flight frame control and adaptive bitrate, same as the desktop). A separate Cargo workspace like `client/src-tauri/` and `web/` |
| `android/app/` | Android app (Kotlin + Jetpack Compose): phone and TV interfaces, MediaCodec decoding / encoding, screen recording, accessibility service (remote control) |
| `client/src/` | Console UI (React 19 + TypeScript, no UI library), shared by the desktop and web consoles, which differ only in `client/src/lib/backend/`; strings in `client/src/locales/` |
| `assets/icon.svg` | Application icon source |

How each host stage is implemented on the three platforms (on Windows they run in the system service, see [Windows service](#windows-service)):

| Stage | Windows | macOS | Linux (X11) |
|---|---|---|---|
| Screen capture | DXGI Desktop Duplication | ScreenCaptureKit | MIT-SHM shared memory + DAMAGE change notifications; RandR for multiple monitors |
| Cursor shape | DXGI | NSCursor | XFIXES |
| Video encoding | Media Foundation hardware encoders, OpenH264 fallback | VideoToolbox hardware encoders (H.264 / HEVC), OpenH264 fallback | OpenH264 |
| Input injection | SendInput (scan codes) | CGEvent (virtual key codes) | XTEST (evdev physical key codes) |
| Audio capture | WASAPI loopback | ScreenCaptureKit (system mix) | PulseAudio / PipeWire output monitor |
| Clipboard | Change detection via the clipboard sequence number | NSPasteboard change count | XFIXES selection-owner change notifications |
| Console UI | WebView2 | WKWebView | WebKitGTK (WebCodecs decoding via GStreamer) |

### Windows service

The same `airhand.exe` plays several roles depending on its command line (`client/src-tauri/src/winsvc/`):

| Process | Runs as | Does |
|---|---|---|
| `airhand.exe` | the signed-in user | Tray and console |
| `airhand.exe --service` | SYSTEM, session 0 | The Airhand service: starts the host in the console session (the one at the physical screen) and watches it; moves it when the console session changes (switching users, signing out) and restarts it if it exits |
| `airhand.exe --agent` | SYSTEM, console session | The host: server connection, capture, encoding, input, audio, clipboard |
| `airhand.exe --install` / `--uninstall` / `--stop-service` | administrator | Called by the installer |

- Desktop Duplication and `SendInput` only work on the desktop the calling thread is on. UAC prompts, the lock screen and the sign-in screen are on the secure `Winlogon` desktop, which only SYSTEM can open; the capture and input threads switch to whichever desktop currently receives input before using it (`client/src-tauri/src/host/desktop.rs`)
- The tray and the host talk over a named pipe per session (`\\.\pipe\airhand-host-<session>`), one JSON message per line. Only interactive users can connect, they can't open another instance of the pipe to impersonate the host, and the tray checks that the pipe belongs to SYSTEM
- The service and the host have no console: they log with `OutputDebugString` (see [Windows service](#windows-service-1) under Development)

### Video

| Stage | How it works |
|---|---|
| Capture | Frames are only produced when the screen changes (DXGI on Windows, ScreenCaptureKit on macOS, DAMAGE on X11), so an idle desktop uses almost no bandwidth |
| Preprocessing | `fast_image_resize` (SIMD) for scaling, `yuvutils-rs` (AVX2) for BGRA→NV12/I420; all buffers are reused |
| Encoding | **Hardware first**: Media Foundation hardware encoders (NVENC / Quick Sync / AMF) for **AV1 / HEVC / H.264**, low-latency mode, no B-frames, very long GOP; on macOS VideoToolbox (H.264 / HEVC, low-latency rate control for H.264). Without one it falls back to **OpenH264 in screen-content mode**. Formats are ranked **AV1 > HEVC > H.264**; the most efficient one both sides support (the host can encode, the controller can decode) is picked automatically |
| Rate control | The controller ACKs every frame. The host caps the number of **in-flight frames** (≈ RTT × fps + 2); on a slow network it waits and **skips to the newest frame** instead of building up lag. Adaptive bitrate steps down under congestion and recovers gradually |
| Transport | QUIC with **BBR** congestion control (steadier than Cubic on lossy links such as Wi-Fi or cross-border routes). Control messages, audio and video are queued separately, so pings and mouse events never wait behind a large frame |
| Decoding | The controller decodes on the GPU with **WebCodecs** (`optimizeForLatency`) and paints straight into a `desynchronized` Canvas |
| Cursor | The remote cursor's **shape** is mirrored to the local cursor and movement is handled locally, so there is **no perceptible cursor lag** |
| Background sessions | Sessions that aren't on screen drop to 5 fps |

Measured locally (RTX GPU, 4K display output at 1080p): hardware AV1 encodes a frame in about 7 ms, H.264 in about 9 ms; OpenH264 software encoding takes about 40 ms.

### Audio

The host's system sound (not the microphone) is streamed to the controller:

| Stage | How it works |
|---|---|
| Capture | Loopback capture of the default output device, converted to 48 kHz stereo; follows the default device when it changes (e.g. you plug in headphones) |
| Encoding | Opus at 96 kbps, 20 ms per packet; **no packets during silence** |
| Transport | Same stream as the video, queued after control messages and before video; when the queue is full packets are dropped instead of building up lag |
| Playback | WebCodecs decoding → AudioWorklet adaptive jitter buffer: about 40 ms on a stable network, growing under jitter and shrinking back once stable. A ±1–2% speed adjustment cancels the clock drift between the two sound cards, and a large backlog is skipped |

Only the session you are looking at plays audio (none do in the multi-screen overview); the other hosts are told to stop capturing.

When the host can't capture sound (no audio output device, or no PulseAudio / PipeWire running on Linux, for example), the controller shows a message, the speaker in the toolbar gets a yellow dot and the sound menu gives the reason. The host retries every 2 seconds, and the message goes away once it recovers.

### A/V sync (optional, off by default)

Audio goes through a jitter buffer while video is shown as soon as it arrives, so by default audio lags video slightly: about **70 ms** in our measurements (same machine, local relay, 4K output at 1080p; 49–78 ms per run). That's barely noticeable while working, but may be when watching video.

With **A/V sync** turned on (speaker menu in the toolbar):

- Video frames and audio packets both carry the host's capture time on the same clock (DXGI present time and WASAPI QPC timestamps). The controller measures the capture → display/playback latency of each stream separately; **the clock offset between the two computers cancels out**, so no time sync is needed
- Normally each frame is delayed until it lines up with the audio. If the video is the slower one (tight bandwidth), the audio is buffered longer instead (up to 300 ms); a single slow frame doesn't trigger this
- Measured residual offset: **−3 to −4 ms** on average (negative = audio slightly early), worst case −31 ms, inside the range generally considered imperceptible (audio ahead by up to about 45 ms)
- Trade-off: video appears about 70 ms later, so typing and dragging feel slower; it suits watching video, not working. The cursor is drawn locally and is unaffected. The toolbar shows "Sync +N ms"

The delay from painting to the Canvas to the pixels lighting up is estimated at 8 ms (not measured with a camera).

### Files on disk

- macOS and Linux: no config file, the server address is compiled in. Windows: only `config.toml` in the install directory (see [Windows installation](#windows-installation)), removed when uninstalling. Logs never go to files: stderr, or `OutputDebugString` for the Windows service
- Host mode **doesn't create a WebView**, so it creates no files at all
- The single-instance lock is a kernel object, not a lock file: a named mutex on Windows, an abstract-namespace Unix socket on Linux, a Mach port name registered with the bootstrap server on macOS
- The console's WebView needs a data directory: it lives in a random subdirectory of the temp directory, runs in private mode (no cookies or cache saved), and is deleted when the window closes or the program exits. If the process is killed, leftovers are removed on the next start. On macOS WKWebView needs no data directory and writes nothing to disk in private mode
- On Linux, the few things that can only be handed over as files (the tray icon for AppIndicator, WebKitGTK's GStreamer plugin cache) live in a random per-process directory under `XDG_RUNTIME_DIR` (a RAM disk) that is removed on exit, including on SIGTERM / SIGINT. The Mesa / NVIDIA on-disk shader caches are turned off
- Controller settings live in memory only
- Android: settings, saved devices and the phone's control code are only stored in the app's private directory, never backed up or transferred, and removed by the system on uninstall (see [Where the data is stored](#where-the-data-is-stored))
- All of the above is about the desktop client. The web console is a service you deploy: it keeps its self-signed HTTPS certificate in a data volume, and the browser also keeps settings in memory only (plus a session cookie when a password is set)

## Security

- TLS 1.3 throughout (QUIC); the client verifies the server by **certificate fingerprint pinning**
- Brute-force protection: at most 20 failed attempts per IP (IPv6 grouped by /64) per 10 minutes, and each failure adds a 400 ms delay
- Resume tokens: when a controller connects, the server gives it a token derived from the host's code and secret (it can't be computed or reversed without the secret), and the controller presents it when reconnecting. Requests with a token don't count as failures and aren't rate limited, so however long a host stays offline, automatic reconnects can't lock the controller (or a web console sharing one IP) out. With a wrong token the server always answers "offline", revealing nothing about whether the code exists, so tokens can't be used to guess codes. A token stops working when the host changes its code
- Getting a control code back after a reconnect requires a random secret, so nobody else can claim it
- Fixed control codes (Windows): the server only stores a hash of the host's secret (in `airhand.db`), so the database can't be used to claim codes; on the host the secret is encrypted with DPAPI for the SYSTEM account, so a copied `config.toml` doesn't carry the code to another computer. **Change control code** releases the old code on the server
- The Windows host runs as SYSTEM so that it can operate UAC prompts and the sign-in screen: whoever has the control code controls the computer with administrator rights. Give the code only to people you trust; whoever sits at the computer sees who is connected and can disconnect them, change the code or quit Airhand
- The host's tray always shows who is controlling the computer; disconnect them or change the code with one click
- The relay server can see the content of session streams (TLS ends at the server), so run it on a machine you trust
- Web console: the WebSocket only accepts pages from the same site (so other websites can't connect through your browser); an access password can be set (HTTP Basic login, then an HttpOnly, SameSite=Strict cookie; wrong passwords are answered with a delay); use HTTPS from other computers. The web console sees the picture too, so run it on a machine you trust. Everyone using it reaches the relay from that machine's IP, so they share the brute-force allowance

## Known limitations

- The host supports Windows and Linux **X11** desktops. Under Wayland only X11 (XWayland) applications can be seen and controlled; native Wayland windows are black and the controller suggests logging in with an X11 (Xorg) session. Full Wayland support needs xdg-desktop-portal screen sharing (the local user has to confirm each time) and isn't implemented
- The Linux host only has OpenH264 software encoding (no VA-API / NVENC yet), so the controller uses H.264
- Decoding on a Linux controller relies on WebKitGTK + GStreamer: H.264 needs `gstreamer1.0-plugins-bad` (parsing) and `gstreamer1.0-libav` or openh264 (decoding); AV1 / HEVC depend on the installed GStreamer decoders. If a format turns out not to produce a picture, the controller switches to the next available format by priority
- The Windows host only runs in the console session (the physical screen): Remote Desktop (RDP) sessions can't be controlled. `Ctrl+Alt+Del` can't be sent yet (it needs `SendSAS`). Debug builds run the host in the tray process, without SYSTEM rights, so they can't capture or control UAC prompts and the lock screen (the controller shows a capture error meanwhile)
- On Linux the lock screen can't be controlled (the screensaver grabs the keyboard and mouse)
- The macOS host needs the Screen & System Audio Recording and Accessibility permissions, which only the local user can grant in System Settings; since macOS 15 the system also asks again from time to time whether to keep allowing screen recording. The login window and the lock screen can't be captured or controlled
- A Mac host doesn't offer AV1 (current Apple chips can only decode it in hardware), so AV1 shows as unavailable in the console and HEVC is used instead
- A macOS controller (WKWebView) can't lock the keyboard: in fullscreen, Command+Tab, Command+Space and the like are still handled locally, so use "Send shortcut" in the toolbar. WKWebView on older macOS versions may lack Opus decoding, in which case the console says remote audio can't be played
- The clipboard syncs text only, not files
- Audio flows from host to controller only; the controller's microphone isn't forwarded. Audio is captured after the volume control, so a muted host sends silence
- Web console: reloading or closing the tab disconnects its sessions; in Firefox, text copied locally isn't synced to the remote side automatically; outside fullscreen some shortcuts (Ctrl+W…) can't reach the remote computer
- Android host: each time sharing starts, screen recording must be confirmed on the phone (a system rule), and sharing can't resume by itself after the system kills the app; remote control uses accessibility gestures, so apps that handle touches themselves (games…) may react less precisely; typing only works in regular text fields (arrow keys and Tab need Android 13+); a secured lock screen and screens marked secure are black; since Android 10 apps can't read the clipboard in the background, so the clipboard only syncs from the controller to the phone; sharing sound needs Android 10+, and some apps don't allow their sound to be captured; HEVC and AV1 are only offered when the phone has a **hardware** encoder for them (few phones have an AV1 hardware encoder so far), otherwise H.264 is used (with the system's software encoder if there's no hardware H.264 encoder)
- Android controller: one device at a time; no A/V sync; TVs only watch; HEVC and AV1 are only used when the device has a **hardware** decoder for them, otherwise H.264
- AV1 on Android (encoding and decoding) hasn't been verified on a real device yet: the emulator has no AV1 hardware codecs

## Development

### Tests

```bash
# repository root: protocol + link library + server
cargo test

# web/: web console (WebSocket protocol, password, HTTPS certificate)
cargo test

# client/: UI (jitter buffer and A/V sync simulations, H.264 conversion, UI strings)
npm test

# client/src-tauri/: client Rust (encoders incl. hardware probing, Opus packing, format conversion…)
cargo test --lib
# audio loopback capture (needs an audio output device; plays a 1.5 s test tone at -60 dBFS, barely audible)
cargo test --lib loopback_captures_tone -- --ignored --nocapture

# android/native/: Android Rust core (runs on this computer: in-flight control, keyframes, audio fan-out, command format)
cargo test
# android/: gestures (trackpad / touch / multi-touch / pass-through), keyboard, control codes, strings (every Rust problem / notice / system action has English and Chinese text)
./gradlew testDebugUnitTest -Pairhand.abis=x86_64
```

Testing the Android host doesn't need a second phone: `android/native` has a protocol-level controller that connects to the phone's code from a computer, counts frames / audio, and can tap and type:

```bash
# android/native/ (an app in the emulator reaches a server on this computer at 10.0.2.2:4433)
cargo run --example probe -- <phone's control code> 8 --tap 0.5,0.5 --text hello --audio
# multi-touch and system actions: pinch out, open quick settings (with "Ask before someone connects" on, allow it on the phone first; timing starts then)
cargo run --example probe -- <phone's control code> 6 --pinch --action QuickSettings
```

On macOS the screen / audio capture tests (`sck_capture`, `loopback_captures_tone`, video and audio in the end-to-end test) and the input injection test (`mac_move_to_center`) need the Screen & System Audio Recording and Accessibility permissions for the terminal that runs them (System Settings → Privacy & Security). Without them `sck_capture`, `loopback_captures_tone` and `mac_move_to_center` skip themselves; the end-to-end test checks that the host reports the missing permission (rather than a capture failure) and then skips the video and audio checks (clipboard, ping, disconnect and the rest are still checked).

End to end (a real server, a real host with screen capture and encoding, and a protocol-level controller):

```bash
# repository root: start a server without certificate pinning
cargo build -p airhand-server
AIRHAND_CERT=target/e2e/cert.pem AIRHAND_KEY=target/e2e/key.pem ./target/debug/airhand-server &

# client/src-tauri/
cargo test --lib e2e_roundtrip -- --ignored --nocapture   # video / codec switching / clipboard / audio / ping / disconnect
cargo test --lib e2e_reconnect -- --ignored --nocapture   # getting the code back after a server restart, changing the code
```

### Debugging tools

```bash
# client/: debug build that starts straight in controller mode; AIRHAND_LOG sets the log level
AIRHAND_DEV_CONTROLLER=1 AIRHAND_LOG=debug npm run tauri dev
# client/: A/V sync measurement (the host plays a white flash + short beep; usage at the top of the script)
node scripts/measure-avsync.mjs <control-code>

# client/src-tauri/: headless host, for testing the controller on the same machine
# (mute the controller, otherwise the played-back audio is captured again and echoes)
cargo run --release --example headless_host
# client/src-tauri/: encoder benchmark (captures the real screen and compares the encoders)
cargo run --release --example encode_bench
```

Use release builds when judging smoothness or A/V sync: debug builds scale very slowly (4K → 1080p takes about 0.5 s per frame).

### Windows service

Debug builds run the host inside the tray process, so `npm run tauri dev` needs no installation. To work on the service itself, install the installer, or register a build directly from an administrator terminal:

```powershell
# client/src-tauri/
target\release\airhand.exe --install       # register and start the service for this airhand.exe (creates config.toml next to it)
target\release\airhand.exe --stop-service  # stop it before rebuilding: the exe is in use
target\release\airhand.exe --uninstall     # remove the service and the startup entry
```

The service and the host log with `OutputDebugString`: run Sysinternals DebugView as administrator and enable Capture → Capture Global Win32. For a different log level, add `AIRHAND_LOG=debug` to the multi-string value `Environment` under `HKLM\SYSTEM\CurrentControlSet\Services\Airhand` and restart the service. While the service is installed, a debug build started from elsewhere still runs its own in-process host (with its own `config.toml` and control code).

### Web console

```bash
# build the UI, then start the service in web/ (connects to a server on 127.0.0.1:4433, serves ../client/dist); open http://localhost:8080
(cd client && npm run build)
cd web && cargo run
# while working on the UI: npm run dev in client/ and open http://localhost:1420 (/ws is proxied to airhand-web on port 8080)
```

The WebSocket protocol between the browser and the service is described at the top of `web/src/console.rs`.

### Linux / WSL

`bash client/scripts/linux-test.sh` runs all of the tests above (unit, X11 capture and input injection, audio loopback, end to end, frontend). The X11 tests open fullscreen windows and inject input, so the script starts its own Xvfb screen and leaves your desktop alone; audio tests are skipped without PulseAudio / PipeWire. It needs `sudo apt install xvfb x11-apps`.

On Windows you can use WSL2 (tested on Ubuntu 24.04; WSLg ships with PulseAudio):

```bash
# Copy the repository to WSL's own disk first (building under /mnt/c or /mnt/d is much slower)
rsync -a --exclude target --exclude node_modules /mnt/d/work/Projects/airhand/ ~/airhand/
cd ~/airhand && bash client/scripts/linux-test.sh
```

- The console works in WSLg: `AIRHAND_DEV_CONTROLLER=1 npm run tauri dev` opens it on the Windows desktop
- To run a host: `Xvfb :99 &`, then `DISPLAY=:99 cargo run --example headless_host` (WSLg's own desktop is Wayland)
- WSL2 uses NAT networking by default: Windows programs reach a server inside WSL through WSL's IP (`ip addr show eth0`), e.g. `AIRHAND_SERVER=172.30.x.x:4433`

### Conventions

#### Protocol versions

Client and server compare `PROTOCOL_VERSION` (`crates/airhand-proto/src/lib.rs`) when they connect. On a mismatch the server replies `Incompatible` and the client tells the user whether to update Airhand or the server, then retries once a minute. Bump the version for any incompatible protocol change. The handshake itself (`ClientMsg::Hello`, `ServerMsg::Welcome` / `Incompatible`, the ALPN) must **never** change, otherwise different versions can't even tell each other that they differ. The `version_handshake_wire_is_stable` test guards this.

#### Messages are types, not text

Anything the user may see that crosses the network or comes from the Rust side is an enum (`ConnectError`, `Notice`, `ByeReason`, `Problem`…). The UI turns it into text in the user's language:

- Console strings live in `client/src/locales/zh.ts` (the full list of keys) and `en.ts`
- Tray strings use `tr!` in `client/src-tauri/src/tray.rs`
- `npm test` fails if a Rust `Problem` / `NoticeView` variant has no string in either language
- Android strings live in `android/app/src/main/res/values/strings.xml` (English, the default) and `values-zh/strings.xml`; `Messages.kt` maps the types to them. `./gradlew testDebugUnitTest` (`MessagesTest`) fails if a Rust `Problem` / `NoticeView` has no Android string, a string for a variant with `detail` doesn't show it, or the two languages differ in keys or placeholders
- Logs and internal errors are English

#### Icon

The source is `assets/icon.svg`. After editing it, run `npx tauri icon ../assets/icon.svg -o src-tauri/icons` in `client/`, then copy `src-tauri/icons/32x32.png` to `src-tauri/icons/tray.png`.
