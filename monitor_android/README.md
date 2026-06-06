# S21 Monitor — native Android client

A native Kotlin/Jetpack-Compose monitor mixer for S21_HiJack. It speaks the
same `/monitor/...` OSC contract as the daemon and the web client, and keeps the
connection alive with the screen off via a foreground service.

This coexists with the Flutter client in [`../monitor_app/`](../monitor_app/) —
neither replaces the other. The web monitor (served by the daemon) remains the
right client for iPhone/iPad, where background persistence is impossible; this
native app exists for Android, where a foreground service *can* hold the mix
alive with the screen locked.

## Build / install

Requires the Android SDK (set `sdk.dir` in `local.properties`) and a JDK 17+
(the Gradle wrapper provisions one). Toolchain mirrors the WFS-DIY remote:
AGP 9.2.1 · Kotlin 2.3.0 · Compose BOM 2026.01.01 · Gradle 9.4.1 · compileSdk 36
· minSdk 24.

```powershell
# Debug APK
.\gradlew.bat assembleDebug
#   -> app\build\outputs\apk\debug\s21-monitor-0.1.0-debug.apk

# Build + install to a connected device
.\gradlew.bat installDebug
```

Close any Android Studio Gradle sync on this module before running CLI builds —
they contend on the Gradle lock.

## Architecture

- `osc/` — `OscCodec` (big-endian OSC 1.0, a port of the Flutter client's codec)
  and `MonitorProtocol` (the address builders + inbound parser; the wire contract).
- `service/MonitorService` — foreground service (`dataSync`) that owns a single
  shared UDP socket (the daemon replies to the source port), a receive thread →
  queue → coroutine processing loop, a 10 s heartbeat + 15 s watchdog, and state
  as a `StateFlow` the UI collects. Modeled on the WFS-DIY remote's `OscService`.
- `discovery/Discovery` — broadcast `/monitor/discover`, collect
  `/monitor/discovered` (captures the daemon's host from the reply source).
- `data/CredentialsStore` — SharedPreferences profile (name / host / port).
- `ui/` — Compose screens (Connection, Monitor: "My Mix" / "My Aux") styled to
  match the web monitor's palette and layout.

## To verify against the real desk

- **Fader range/taper** — currently a linear −80…+10 dB map
  (`FADER_MIN_DB`/`FADER_MAX_DB` in `ui/widgets/Faders.kt`). Adjust if the desk's
  send/aux levels don't match.
- **Screen-off persistence** — if Android kills it when locked, exempt the app
  from battery optimization (some OEMs are aggressive).
- **Discovery** — broadcast may be blocked on some APs; manual IP entry is the
  fallback.
- **Names / permitted auxes** populating, and cross-client **echo**.

## Before public distribution

- Add a release signing config (keystore + `key.properties`) — the release build
  is debug-signed for now.
- Turn on R8 and drop `material-icons-extended` (only two icons are used) to
  shrink the APK.
- The `applicationId` is `com.pob31.s21monitor`.
