# Install & file association

Show files use the `.s21show` extension. Once associations are
installed, double-clicking a `.s21show` file in your file manager
launches the app and loads the show automatically. From the command
line, `s21_hijack path/to/show.s21show` does the same thing.

The format is JSON internally; legacy `.json` show files still load
without conversion.

---

## Linux

The repo's `assets/` directory has the two files needed:

- `assets/s21_hijack.desktop` — desktop entry advertising the MIME type.
- `assets/s21_hijack-mime.xml` — MIME-info package mapping `*.s21show` →
  `application/x-s21show`.

Per-user install (no root):

```bash
# Install the binary somewhere on PATH.
cargo install --path . --bin s21_hijack

# Register the MIME type.
mkdir -p ~/.local/share/mime/packages
cp assets/s21_hijack-mime.xml ~/.local/share/mime/packages/
update-mime-database ~/.local/share/mime

# Register the desktop entry.
mkdir -p ~/.local/share/applications
cp assets/s21_hijack.desktop ~/.local/share/applications/
update-desktop-database ~/.local/share/applications

# Set as the default opener for .s21show files.
xdg-mime default s21_hijack.desktop application/x-s21show
```

For a system-wide install replace `~/.local/share` with
`/usr/share` and use `sudo`. After install, log out and back in (or
restart your file manager) to pick up the new association.

---

## Windows

If you're using **Inno Setup** for the installer, drop
`assets/s21_hijack_assoc.iss` into your `.iss` script — it has the
`[Registry]` entries to register the extension with the installed
binary. Inno picks `HKLM` vs `HKCU` automatically based on your
`PrivilegesRequired` setting.

```iss
; In your .iss file:
#include "path\to\s21_hijack_assoc.iss"
```

Or copy its `[Registry]` section verbatim into your existing
`[Registry]` block.

For a manual one-off install without an installer, use the
ready-made `.reg` template:

1. Open `assets/s21_hijack_register.reg` in a text editor.
2. Replace `C:\\Path\\To\\s21_hijack.exe` (two locations) with the
   absolute path to your installed binary.
3. Double-click the file in Explorer (or `regedit /s
   s21_hijack_register.reg`). It writes to `HKEY_CURRENT_USER` so
   no admin elevation is required; the association is per-user.

---

## macOS

There's no `.app` bundle build today. When packaging lands, the
relevant `Info.plist` fragment for `CFBundleDocumentTypes` is:

```xml
<key>CFBundleDocumentTypes</key>
<array>
    <dict>
        <key>CFBundleTypeName</key>
        <string>S21 HiJack Show</string>
        <key>CFBundleTypeRole</key>
        <string>Editor</string>
        <key>LSItemContentTypes</key>
        <array>
            <string>com.s21hijack.show</string>
        </array>
        <key>LSHandlerRank</key>
        <string>Owner</string>
    </dict>
</array>
<key>UTExportedTypeDeclarations</key>
<array>
    <dict>
        <key>UTTypeIdentifier</key>
        <string>com.s21hijack.show</string>
        <key>UTTypeDescription</key>
        <string>S21 HiJack show file</string>
        <key>UTTypeConformsTo</key>
        <array>
            <string>public.json</string>
        </array>
        <key>UTTypeTagSpecification</key>
        <dict>
            <key>public.filename-extension</key>
            <array>
                <string>s21show</string>
            </array>
            <key>public.mime-type</key>
            <array>
                <string>application/x-s21show</string>
            </array>
        </dict>
    </dict>
</array>
```

Until the bundle exists, `cargo run -- path/to/show.s21show` still
auto-loads the show; the file-manager double-click flow needs the
`.app`.

---

## CLI usage

The optional positional argument:

```
s21_hijack [SHOW_FILE]
```

When provided, the path is pre-populated in the Setup tab's "Show
file:" field and the show is loaded automatically on the first
frame. Combine with the existing flags for full control:

```
s21_hijack --console-ip 192.168.1.10 --mode mode2 my_gig.s21show
```

---

## Help-bubble translations

The interface is English-only by design, but the hover **help bubbles**
(tooltips) can be localised. English is always the reference and is shown
whenever a translation is missing — so a partial translation is fine, and the
app works with no translation files at all.

Translations are plain JSON files, one per language. At startup the app scans
these locations, with later ones overriding earlier (so a machine-specific file
can override a shipped translation):

1. **Beside the binary** — a `locales/` folder next to the executable. This is
   where translations ship for distribution.
2. **Project tree** — `assets/locales/` (relative to the working directory),
   used when running from source with `cargo run`. The repo ships
   `template.json` (the English reference) and an example `fr.json` here.
3. **User config** — a `locales/` folder beside `preferences.json`, for
   per-machine overrides:
   - **Windows:** `%APPDATA%\s21_hijack\locales\`
   - **Linux:** `~/.config/s21_hijack/locales/`
   - **macOS:** `~/Library/Application Support/s21_hijack/locales/`

Each file is named with a language code (e.g. `fr.json`) and maps help keys to
translated text. Start from the reference template — copy `template.json` (the
full English key list, in `assets/locales/` and shipped beside the binary) to a
new `<code>.json`, or regenerate it from the app:

```
s21_hijack --dump-help-template > fr.json
```

Then edit `fr.json`: set `"_name"` to the language's display name (shown in the
picker) and translate each value. Leave the keys unchanged — they are the
stable identifiers the app looks up. Any key you omit falls back to English.

```json
{
  "_name": "Français",
  "cue.go": "Rappeler le cue suivant.",
  "cue.undo": "Annuler le dernier rappel de cue / snapshot."
}
```

Drop the file into one of those folders, restart the app, and pick the language
under **Setup → Advanced… → Help bubbles**. Adding or editing a language never
requires rebuilding the app. `en.json` and `template.json` are ignored — they
are references, not selectable languages.

Note: a language renders only if a bundled font covers its script. The bundled
Noto Sans covers Latin, Cyrillic and Greek (Western-European languages render
as-is), and per-language Noto Sans CJK subsets cover **Chinese, Japanese and
Korean** (the app swaps to the matching subset when you pick zh/ja/ko). Other
non-Latin scripts (Arabic, Hebrew, Thai, …) would need a covering font added to
`src/ui/fonts.rs` first. If you edit the zh/ja/ko text, regenerate the CJK
subsets with `python scripts/subset_cjk.py` (needs `fonttools`) so any new
glyphs are embedded.
