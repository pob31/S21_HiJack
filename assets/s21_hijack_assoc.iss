; ----------------------------------------------------------------------
; S21 HiJack — Inno Setup file-association fragment
;
; Drop this `[Registry]` block into your existing `.iss` script.
; It registers the .s21show extension with the installed binary so
; double-clicking a show file in Explorer opens it in the app. Uses
; `HKA` (HKEY_AUTO) so installs go to HKLM under admin and HKCU
; otherwise — driven by Inno's PrivilegesRequired setting.
; ----------------------------------------------------------------------

[Registry]
; .s21show extension → ProgID
Root: HKA; Subkey: "Software\Classes\.s21show"; ValueType: string; ValueName: ""; ValueData: "S21HiJack.Show"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.s21show"; ValueType: string; ValueName: "Content Type"; ValueData: "application/x-s21show"; Flags: uninsdeletevalue

; ProgID metadata
Root: HKA; Subkey: "Software\Classes\S21HiJack.Show"; ValueType: string; ValueName: ""; ValueData: "S21 HiJack show file"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\S21HiJack.Show\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\s21_hijack.exe,0"

; Open command — %1 is the file the user double-clicked.
Root: HKA; Subkey: "Software\Classes\S21HiJack.Show\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\s21_hijack.exe"" ""%1"""
