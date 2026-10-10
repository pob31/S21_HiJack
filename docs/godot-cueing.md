# Cueing with Go.dot

S21_HiJack writes cues into a cue system and is driven back by it. It has done that with QLab
from the start; it can now do it with **Go.dot** instead, through a protocol made for the job.
Added in v0.3.0.
The protocol is Go.dot's: `docs/godot-authoring-protocol-0.1.md` in the Go.dot repository is
the contract, and this page is what S21_HiJack does with it (its §7.2 and §7.3). The names of
the Go.dot commands, the answer addresses, the on-screen words and the template names are draft
names until the author renames them.

## The toggle

**Setup → Cueing system: QLab | Go.dot.** Saved in the show (show format 20). A show saved
before the toggle existed opens as a QLab show. Like the other connection settings, it can only
be changed while disconnected, and it is greyed out in Live Music mode, which has no Snapshots
tab.

| | QLab | Go.dot |
|---|---|---|
| Where cues are written | QLab's IP and port (53000), with an app patch and a console patch | Go.dot's IP and port (**8010**) |
| What a snapshot becomes | a group of network cues, one message each | **one** OSC cue holding every message |
| How the cue is made | `/new`, `/cue/selected/*`, a `uniqueID` query and `/move`, 30 ms apart | one `cue.capture` datagram (or a few, for a long one) |
| Exporting again | makes another group | **updates the same cue** |
| Per-cue triggers | the QLab templates (`/go`, `/stop`, ...) | the Go.dot templates (below) |
| Macro cue steps | sent to QLab | sent to Go.dot, translated |

Everything QLab does is unchanged when the toggle says QLab.

## What connecting does

When the show's cueing system is Go.dot, connecting the console also **declares the app in
Go.dot** as a device:

```
/godot/cmd/mount/declare  "/s21"  <trigger port>  <web port>  "S21_HiJack"
```

- **`/s21`** is the app's root. The trigger listener takes every trigger address under it as
  well as bare: `/s21/cue/go` and `/cue/go` both fire GO, so Go.dot's cues (aimed at the device
  `/s21`) and QLab's both reach it.
- **The trigger port** (53001) is where Go.dot sends the cues aimed at the app, and where its
  **answers** arrive (`/godot/declared`, `/godot/described`, `/godot/captured`). So Go.dot
  captures only work while the app is connected: the trigger listener runs with the connection.
- **The web port** (8080) is where the app serves its **menu** (below); 0 when the web server is
  off or did not start, and Go.dot then offers no menus for the app.

The outcome is the Setup status line: *Declared in Go.dot as /s21 (created)*, then *Declared in
Go.dot as /s21: its menu has 9 nodes*, or why not. The app **declares again** whenever its menu
changes (a snapshot stored, renamed or deleted, a cue renumbered, a macro added), comparing once
a second, and tries again every 10 s after a failed declare (Go.dot started after the app). A
Pad-only connection (SD / Quantum) starts no trigger listener and does not declare.

If Go.dot's OSC filter is set to "registered only", a datagram from a host Go.dot does not know
is dropped before it is read: declare before switching that on, or add this computer in Go.dot's
Network tab.

**Headless** does the same when the loaded show's cueing system is Go.dot, and writes the
outcome to the log. `--godot-ip` and `--godot-port` override the address saved in the show.

## The menu: `GET /s21` on the web port

The app describes itself as an OSCQuery tree, which Go.dot fetches when the app declares, and
offers in its OSC cue editor as cascaded menus (`snapshot` → `recall` → the snapshot names):

| Node | TYPE | Menu |
|---|---|---|
| `/s21/snapshot/recall` | `s` | the snapshot names |
| `/s21/snapshot/recall_full` | `s` | the snapshot names |
| `/s21/cue/go` | none | |
| `/s21/cue/previous` | none | |
| `/s21/cue/fire` | `f` | the cue numbers |
| `/s21/macro/fire` | `s` | the macro names |

`curl -s http://127.0.0.1:8080/s21` shows it; a sub-path (`/s21/cue`) answers its subtree. The
route sits behind the web server's source allowlist like the rest of it, and answers with
`Connection: close`, as Go.dot reads to the end of the connection.

## Capturing a snapshot into Go.dot

Under Go.dot the Snapshots tab's export row holds two buttons:

- **Go.dot recall cue** — one OSC cue aimed at the app, `/s21/snapshot/recall "<snapshot id>"`.
  Firing it in Go.dot recalls the snapshot here. It carries the snapshot's **id**, not its name,
  so renaming the snapshot does not break it (the listener takes an id first, a name second).
  Tick **ignore scope** first for `/s21/snapshot/recall_full`, which recalls every stored
  parameter.
- **Go.dot console capture** — one OSC cue holding every stored parameter, straight at the
  console's GP OSC addresses (`/channel/<n>/fader f:-10` ...), with palette overrides applied and
  iPad-only parameters left out, as QLab's export does.

Each lands **after the standby** of Go.dot's focused list. The first capture draws the cue's
identifier and the app keeps it on the snapshot (saved with the show), so every later capture of
that snapshot **updates the same cue** wherever it now sits; a cue deleted in Go.dot meanwhile is
made again after the standby. One identifier per snapshot: whichever export ran last, recall or
console, is the snapshot's cue in Go.dot. The status line says what Go.dot answered (*Go.dot
recall cue created: Recall 'Verse 1' (1 message)*), or Go.dot's reason for refusing (`locked`,
`unknown-id`, ...), or that it did not answer within 2 s.

A long console capture is sent in several datagrams of at most 1,200 bytes, the first carrying
the cue's head, the rest appending to the same cue; each waits for Go.dot's answer.

### Declaring the console in Go.dot, by hand

The console capture's messages go straight to the desk, so Go.dot needs a device for it. The app
does not declare the console (it is not the console). In Go.dot's Network tab, add a device with:

- **prefixes** `/channel /console /digico`,
- the console's IP and its **GP OSC port** (the one in Setup → Console),
- Rx and Tx on.

Without it, Go.dot refuses the capture with `unknown-id` (no device for those addresses).

## Triggers: S21 cues that drive Go.dot

In the cue editor, **+ from template…** offers seven Go.dot templates. Each fires when the S21 cue
is recalled, after the console row and snapshot overlay, and goes to the Go.dot address in Setup:

| Template | Sends |
|---|---|
| Go.dot: GO | `/godot/cmd/go` |
| Go.dot: fire this snapshot's cue | `/godot/cmd/cue/fire "<id>"` |
| Go.dot: GO at this snapshot's cue | bundle `/godot/cmd/standby/set "<id>"`, `/godot/cmd/go` |
| Go.dot: fire cue [N] | `/godot/cmd/cue/fireNumber "N"` |
| Go.dot: GO cue [N] | bundle `/godot/cmd/standby/setNumber "N"`, `/godot/cmd/go` |
| Go.dot: Esc | `/godot/cmd/run/stopAll` |
| Go.dot: double Esc | `/godot/cmd/run/killAll` |

- **"This snapshot's cue"** is the Go.dot cue the S21 cue's snapshot was captured into, read when
  the trigger fires. A snapshot never captured into Go.dot sends nothing, and the log says so.
- **A number is exact text.** Go.dot compares cue numbers as written: `12` is not `12.0`. Type
  the number as Go.dot shows it.
- A Go.dot row in the editor shows its verb as a menu, and a cue number field for the `[N]`
  verbs.
- Go.dot triggers need Go.dot to be the show's cueing system; in a QLab show they are skipped
  with a log line.

## Macro cue steps

The macro steps QLab Go, QLab Go Cue #, QLab Stop and QLab Panic follow the cueing system. Under
Go.dot:

| Step | Sends to Go.dot |
|---|---|
| Go | `go` |
| Go Cue # N | bundle `standby/setNumber "N"`, `go` |
| Stop | `run/stopAll` (Esc) |
| Panic | `run/killAll` (double Esc) |
| Pause, Resume | nothing: Go.dot has no pause or resume of everything, and the step fails with a sentence saying so |

Headless sends these to Go.dot as well in a Go.dot show (a QLab show still drops them headless,
as it always has). The Macros tab still names the steps "QLab ...".
