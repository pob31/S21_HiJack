# Verifying S21 HiJack against an SD / Quantum console

Thank you for lending us console time — this is the one thing we genuinely
cannot do ourselves.

S21 HiJack has driven DiGiCo S21/S31 desks for a while. Everything in this
build that talks to an SD or Quantum console was written against DiGiCo's
published documentation and community reverse-engineering, and **not one
value in it has ever been checked on real hardware.** This session is where
that changes.

There is one question that matters more than all the others, and it is
question **(c)** in the probe below. Everything else is refinement.

---

## 1. Read this first — the app writes to your console

S21 HiJack is a control application. It **sends changes to the desk**: fader
moves, mutes, sends. The probe deliberately writes values in order to find out
whether the desk acts on them.

Please therefore:

- **Do not run this during a show, a rehearsal, or a soundcheck.** A
  maintenance window or a spare desk is ideal.
- **Save your session first**, and preferably load a throwaway session or a
  fresh one. Assume any parameter this app touches may end up at an
  unexpected value.
- Keep the probe to channels you do not mind disturbing. The built-in probe
  targets **input channel 1** unless you change it — put something harmless
  there, or point the probe elsewhere.
- If anything at all looks wrong, **press Disconnect in the app**. That drops
  every socket immediately; the console is then on its own again.

Nothing here can modify your console's firmware, its licences or its stored
sessions. The worst realistic outcome is parameters left at odd values in the
loaded session, which reloading your saved session undoes.

---

## 2. What you need

- An SD or Quantum console you can play with, with **External Control**
  available (it needs DiGiCo's activator software installed on the console —
  if the External Control page is missing or greyed out, that is why).
- A Windows PC on the **same network** as the console.
- The release zip we sent you.
- 45–60 minutes, unhurried.
- Optional but very welcome: a phone to film anything surprising.

Please note down, and send back with your results:

- Console **model** (e.g. SD9, Quantum 338) and **software version**
  (Master screen → Diagnostics → About, or the version shown at boot).
- Whether the console has ever had a DiGiCo Pad / iPad app connected before.

The software version matters a great deal. DiGiCo's general-purpose OSC
arrived long after the command list we worked from was published, so
"which version" may well be the explanation for anything that behaves oddly.

---

## 3. Set up the console

You will add **two** external-control devices. They are independent — one is
the surface the app connects to, the other is what we want to measure
alongside it. If you only have time for one, do the **DiGiCo Pad** one.

### 3a. The DiGiCo Pad device (required — this is what the app connects to)

On the console: **Master Screen → Setup → External Control**.

1. Enable External Control (top of the page).
2. **Add Device** → choose **DiGiCo Pad**.
3. Enter the **IP address of the Windows PC** running S21 HiJack.
   (This field is labelled for an iPad. The app stands in for the iPad, so it
   takes the PC's address.)
4. Set the ports. These are stated from the **console's** point of view:
   - console **Send** = **9000**
   - console **Receive** = **8000**
5. Tick **Enable** on the device row.
6. **Load the command set.** There is a command-set selector for the device —
   choose the one for your console (`ipad_Q` for Quantum, `ipad_SDv2` for the
   SD range) and press **Load**. This step cannot be automated and is easy to
   miss; on a console that has never had an iPad attached, the device will
   error until it is done.
7. Note the console's **Local IP** shown on this page — you need it in the app.

> **Only one Pad device may be connected at a time.** If you use the DiGiCo
> iPad app, please **close it / disable its device row** for this session, or
> it and S21 HiJack will fight over the same slot.

### 3b. The "other OSC" device (optional but valuable)

Still on **External Control**: **Add Device** → **other OSC** (wording varies
by version). Give it the PC's IP and note the ports it uses — pick anything
free; the app lets you type them in.

On older software this entry reads *"not yet implemented"*. If that is what
you see, skip this section entirely and **tell us** — that answer is itself
useful data.

---

## 4. Set up and connect the app

1. Unzip the release somewhere ordinary (Desktop is fine). Keep the
   `locales` folder **next to** `s21_hijack.exe` — the app reads its help
   text from there.
2. Run `s21_hijack.exe`. Windows SmartScreen may warn that the publisher is
   unknown: *More info → Run anyway*. The build is unsigned; that is expected.
3. On the **Setup** tab:
   - **Console Family** → pick **SD Range** or **Quantum**.
     Selecting this re-stocks the ports below to 8000 / 9000 for you (it will
     not overwrite anything you have typed yourself).
   - **Connection Mode** will grey out. That is correct — the mode is an
     S-series concept and does not apply to your console.
   - **Console IP** → the console's Local IP from step 3a.7.
   - **Console Port** → **8000** (where the console listens).
   - **Local Port** → **9000** (where we listen).
   - **Local IP / interface** → pick the PC network card that faces the
     console, if you have more than one.
4. Press and hold **Connect**.

### What you should see, and what it means

The status dot is deliberately honest, so read it carefully:

| Dot | Meaning |
|---|---|
| **Red** | Not connected. |
| **Yellow** | We are bound to the socket, but **the console has not answered anything yet.** |
| **Green** | The console has replied to us. This is the one that proves two-way communication. |

**A dot that stays yellow forever means the desk never answered.** That is a
real result, not a bug in the app — go to Troubleshooting (§8) before
concluding anything else.

Against a desk that answers, this all happens in well under a second. If the
console stays silent the app waits about five seconds before giving up and
settling on yellow. You should then see the console's **name and serial**
appear, sensible **channel counts**, and a green progress line as it reads the
desk's current values one parameter at a time.

Once that settles, please sanity-check a couple of things and note the answers:

- Do the **channel counts** match your actual console — inputs, and especially
  the **aux and group counts**? We know the S21 reports those two only as a
  courtesy it was never asked for, so they are the counts most likely to come
  back wrong on a different console generation.
- Move a **fader on the app's Inspector/OSC Log view** — does the console
  react? (This is probe (a), done informally.)
- Move a **fader on the console** — does anything appear in the app's OSC Log?
  (This is probe (c), the important one, done informally.)

---

## 5. The Protocol Probe — the actual experiment

Enable it first: **Setup → Advanced settings → Diagnostics → Show diagnostic
tabs**. A **Probe** tab appears at the end of the tab bar (alongside OSC Log
and Inspector).

The probe runs the same three tests against each surface. **Do them in
order** — each one only makes sense if the previous passed.

### (a) Write — "does the desk act on what we send?"

The app sends a known value. **You watch the console.** Did the fader move,
the mute light, the name change?

Record what you saw. If nothing happened, the remaining tests on that surface
will be meaningless, so check §8 first.

### (b) Pull — "can we ask the desk for a value?"

The app sends the parameter path with `/?` appended and no value, then waits.

DiGiCo does not document this for the `/sd/` surface at all — a community
report says it works anyway, and this test is how we find out. Whatever comes
back, the probe records **the value, its type, and its scaling**, which is how
we learn whether that surface talks in dB or in 0–1.

### (c) Push — **the decisive test**

Arm the test in the app, then **go and move that control on the console
itself** — physically, on the surface. Then look at the app.

The probe reports whatever arrived unprompted, how quickly, and whether it was
one message or a burst.

**Why this one matters more than the rest:** S21 HiJack is a live mirror of
the console. Gang a pair of channels and moving one must move the other; that
only works if the desk *tells* us when someone touches it. Being able to *ask*
for values (test b) is enough to read the desk once at connect — it is not
enough to follow it. We cannot poll our way around this: reacting to a fader
ride needs to be near-instant, and even sixty faders polled fast enough is
over a thousand queries a second at a desk that reportedly drops bursts.

So: **a surface that fails (c) cannot drive ganging, pan link or personal
monitoring**, no matter how well (a) and (b) work. If push works on the
general-purpose `/sd/` surface, that is much the best outcome for everyone —
it is DiGiCo-documented, has no one-device-at-a-time limit, and you keep your
iPad.

**"Nothing arrived" is a genuine finding, not a failed test.** Please record
it as deliberately as a success. The worst possible outcome for us is a blank
we cannot tell apart from a step you did not get to.

### Then: the wire-detail probes

With the free-form row in the Probe tab you can settle the small stuff that
decides whether our numbers come out right. Each has an expected answer we
have **guessed**; we want to know if the guess is wrong:

| What | Why we are unsure |
|---|---|
| EQ band numbering, **including bands 5–8 on Aux / Group / Matrix outputs** | Both DiGiCo documents say outputs carry 8 bands where inputs carry 4. We have never seen bands 5–8 on a wire. |
| Whether EQ bands are numbered in reverse | They are on the S21. We assume SD/Quantum are not. |
| Dynamics band identity | The S21 swaps two of them. Again, assumed not here. |
| Booleans: `0/1` as float, as integer, or true/false | Assumed float. |
| Pan: `0…1` or `-1…+1` | Assumed `0…1`. |
| Fader scaling: dB, or normalised `0…1` | The `/sd/` surface is documented as normalised; the Pad surface as dB. Both need confirming, and the **shape** of the normalised curve is pure guesswork on our side. |
| Control-group numbering from 0 or 1 | Assumed from 0. |

Take the ones you have patience for, in that order. The first is the most
valuable.

---

## 6. Sending the results back

In the Probe tab: **Save report…**, and send us the file. It is plain
Markdown; do read it before sending if you would like to see what you are
sharing. It contains your console's name, serial and session name, the probe
results, and any notes you type in.

Please also send:

- The **console model and software version** from §2.
- Anything from §4's sanity checks.
- An **OSC Log** excerpt around anything surprising (the OSC Log tab shows
  every message in and out).
- Your impressions in plain words. "The fader jumped to the top and stuck" is
  more useful to us than a clean log.

If the app crashed or hung, that is worth reporting on its own — with what you
were doing at the time.

---

## 7. What this build does and does not do

This is a first hardware build, deliberately narrow.

**Working on SD / Quantum in this build**

- Connecting, reading the desk's configuration, and mirroring its values.
- Ganging and pan link.
- The fader sidecar (the feature that started all this).
- Recalling snapshots and running macros *from the app*.
- The Probe, OSC Log and Inspector diagnostics.

**Deliberately not in this build yet**

- Personal monitoring and the web remote.
- Following the console's own snapshot changes.
- Inbound cue triggers from QLab / LiveProfessor / MIDI.
- Live palette absorption.
- Headless (no-GUI) operation.

None of these are hard to add — they are S-series-proven code paths held back
until the connection underneath them is confirmed on real hardware. Which is
what this session is for.

---

## 8. Troubleshooting

**The status dot never goes green.**
The console has not answered. In likely order:

1. The **command set was not Loaded** for the Pad device (§3a.6). This is
   the single most common cause and the app cannot detect it.
2. **Ports crossed.** The console's *Send* must be our *Local Port* (9000)
   and the console's *Receive* our *Console Port* (8000). They are stated
   from opposite points of view, which makes this very easy to get backwards.
3. **The DiGiCo iPad app is still connected**, holding the one available slot.
4. **The device row is not ticked Enable**, or External Control is off.
5. **A firewall** is eating inbound UDP. Windows Defender prompts on first
   run — if you dismissed it, allow `s21_hijack.exe` on private networks.
6. **Wrong network card.** If the PC has several, set Local IP explicitly.

**Values appear but look wrong** (faders at the wrong level, EQ on the wrong
band). Very plausible — that is exactly what §5's wire-detail probes are for.
Please record what you see rather than working around it.

**The app connects but the console does not react to it.** That is probe (a)
failing, and it is interesting. Please check the OSC Log to see whether we are
sending at all, and tell us.

**Something else entirely.** Write it down in plain words and send it. At this
stage of the project every surprise is worth more than a clean run.

---

Thank you again. Whatever comes back — including "none of it worked" — is the
first real measurement anyone on this project has had, and it decides what we
build next.
