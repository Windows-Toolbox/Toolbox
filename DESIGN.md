# Windows Toolbox in Rust – design

Status: **proposal**, nothing of this is built yet. It describes where the Toolbox should end up, why, and in which
order to get there without ever having a broken Toolbox in between.

Decisions already taken (2026-11-15):

- Everything moves to Rust, **step by step**: the Toolbox first, the programs one after the other. At the end there
  is no Python runtime any more.
- The look is the one of the **Last Bastion map editor**: egui (panels, lists, fields).
- Installing and starting should become noticeably faster.
- **Settings of all programs are shown and changed in the Toolbox window** (like Steam's library) and take effect
  in the running program immediately. The program defines its page; the Toolbox displays it (section 4.4).
- **One tray icon**: all programs, the Hardware Monitor included, sit under the Toolbox's icon (section 6).
- **Install = unpack the release zip.** No setup program (section 9).
- **Every program and script is its own git project** with its settings in its own folder. This repo holds only
  the Toolbox and the shared library, so it stays small (section 3).
- **A small C# bridge** is the one piece of own code that is not Rust (section 10).
- **The folder is tidy**: sources and generated files never mix; a handful of files per program (section 3).
- **LibreHardwareMonitor is not rewritten.** It stays the developers' library, unmodified and updatable; only our
  own code moves to Rust (section 10).

- **Only the bridge runs with admin rights**, never a program of ours as a whole. The bridge and
  LibreHardwareMonitor are installed into a folder that only admins can write to (section 10).
- **Everything optional is opt-in** (section 2).
- **A program only ever sends its state**; nothing is answered, so replies and updates cannot be mixed up (4.3).
- **At most one process per program holds its pipe and writes its settings** – stopped or running (section 5).

Things marked **(verify)** are assumptions that the first milestone (M0, a throw-away prototype) has to confirm
before anything is built on top of them.

This file is a concept: what, why, in which order. It is not yet a build manual – section 14 says what is still
missing for that and how to work from it.

---

## 1. What is wrong today

The Toolbox works, but its structure makes small changes expensive. The concrete causes, taken from the code:

| Pain | Where it comes from |
|---|---|
| A new setting of the Hardware Monitor touches 4 places | `DEFAULT_SETTINGS` + check in `load_settings()` + `defaults`/`layout()` in the page + README (EXPLANATION.md 6.4) |
| Values must be identical in several files | ports in `program.json`, Python and `setup.ps1`; task name; default settings; device names (EXPLANATION.md 7) |
| Programs are not independent | A program's page is a Python file that the Toolbox imports into **its own process** (`importlib` in `toolbox.py`). Programs in turn import `base`, `i18n`, `storage`, `winutil` from `toolbox\` via `sys.path`. Neither side runs without the other's files. |
| Every program needs two generated `.exe` | starter (`starter.cs`, compiled with `csc.exe`) + renamed `pythonw.exe` (`make_exe.py`), only so that it can be double-clicked and has a name in Task Manager |
| Setup is big and slow | `setup.ps1` is 57 KB; it installs Python via winget, copies it (`runtime\`: 3273 files, 108 MB), installs pip packages, builds the starters |
| Windows helpers are slow | task and autostart queries go through hidden PowerShell processes (`winutil.py`) |
| GUI limits | Tk scroll stripes (the planned PySide6 port), thread rules ("never touch widgets from a thread"), GIL workarounds for pythonnet |

The common root: **the Toolbox is a framework that programs live inside of.** The redesign turns that around.

---

## 2. Principles

1. **A program is an `.exe` that needs nothing from the Toolbox.** Its settings page, its settings file, its tray
   menu, its autostart, its own setup steps – all inside the program. Delete the Toolbox and every program still
   works. (Steam model: Steam lists, starts and updates games; it is not part of them.)
2. **The Toolbox is a library/launcher.** It finds manifests, shows status, starts/stops, **displays each program's
   settings page**, installs/removes, updates. It contains no code of any program – a page arrives as data from the
   program (4.4).
3. **Shared code is linked in, not looked up at runtime.** Programs written here use one library crate
   (`toolbox-kit`) for theme, settings, texts, tray, IPC, autostart. It is compiled into each `.exe`.
4. **Every fact exists once.** No ports, task names or defaults that have to match somewhere else – they are derived
   from the program's `id` or live in one Rust struct.
5. **Idle costs nothing.** A background process blocks in the Windows message loop (0 % CPU) and has no GUI library
   loaded. Windows exist only while they are open.
6. **User functions are files and buttons** – never command-line switches. (Internal arguments between our own
   processes are fine; the user never types them.)
7. **Opt-in.** Everything optional is off by default and can be switched on: start with Windows (Toolbox and every
   program), temperatures in the taskbar, looking for updates, shortcuts, anything that touches the network or
   Windows outside the program's folder. A freshly unpacked program does nothing nobody asked for. The kit enforces
   the easy half: a switch in a settings struct starts as `false` unless written otherwise.
8. **Portable stays.** Everything lives in the folder; moving the folder and starting `Toolbox.exe` repairs what
   Windows remembers by path (autostart, shortcuts). A path is repaired **only when the old target no longer
   exists** – so two copies of the same program (your own build and a release) do not take the entry away from
   each other, and moving a folder still works.

---

## 3. The big picture

```
                       ┌────────────────────────── Toolbox.exe ──────────────────────────┐
                       │  agent role (always running)        window role (while open)    │
   tray icon ◄─────────┤  Win32 message loop, tray menu  ───►  egui window: library,     │
                       │  no egui, no GPU, 0 % CPU             status, install, update   │
                       └───────────────┬─────────────────────────────┬───────────────────┘
                                       │ reads                       │ talks to
                                       ▼                             ▼
                         program.json of every             \\.\pipe\windows-toolbox.<id>
                         added program folder              status · page · set · action · tray · quit
                                                                     ▲
        ┌──────────────────────┬─────────────────────┬───────────────┴──────┬──────────────────────┐
        │ HardwareMonitor.exe  │ DiscordPlus.exe     │ CubeGame.exe         │ LastBastion.exe      │ rayman_borderless.ps1
        │ kit program          │ kit program         │ kit program          │ external exe         │ script
        │ background + window  │ background + window │ just a game          │ no kit               │ no code of ours
        └──────────────────────┴─────────────────────┴──────────────────────┴──────────────────────┘
                                   each one its own folder and git repo, anywhere on disk
```

### Folder layout (target)

Today sources, generated files, downloads and your settings lie next to each other (`Setup.exe`, starters,
`runtime\`, `lhm\`, `data\` between the `.py` files), and a program is spread over many small files (`*_gui.py`,
`*_settings.py`, `i18n.json`, icon, starter, logo script …). Three rules fix that:

1. **This repo is only the Toolbox and the shared library.** No program lives here. It stays small no matter how
   many programs exist.
2. **Every program and script is a git repo of its own**, and all of them look the same: a handful of entries.
3. **A repo contains only what you write.** Everything a build or a running program creates goes to that repo's
   `dist\` (ignored by git) – that folder *is* the installed program.

```
windows-toolbox/                this repo
├─ README.md
├─ DESIGN.md
├─ Cargo.toml                   lists the crates below so one command builds them (Cargo.lock: pinned versions, written by cargo)
├─ build.ps1                    release build → dist\ (the trick from last-bastion: a running exe is renamed aside)
├─ release.ps1                  build + zip + GitHub release (section 9)
├─ kit/                         toolbox-kit: what programs and the Toolbox share (section 5)
├─ toolbox/                     the Toolbox → Toolbox.exe   (src\, assets\: icon and i18n.json, compiled in)
├─ template/                    copy this folder to start a new program
└─ dist/                        (ignored) Toolbox.exe + data\ with its settings.json and log.txt
```

The programs lie wherever you like. Practical is one folder for all of them – the name is yours, here
`toolbox-programs\` – with one repo per program below it:

```
toolbox-programs/               just a folder, not a repo
├─ .cargo/config.toml           one small file for all repos below: shared build folder, local kit (see below)
├─ target/                      the shared build folder (created by cargo)
├─ windows-toolbox/             this repo (above) – best placed here too, so it uses the shared build folder
├─ hardware-monitor/            its own git repo
│  ├─ Cargo.toml                also holds the manifest: [package.metadata.toolbox] (4.1)
│  ├─ README.md
│  ├─ build.ps1
│  ├─ release.ps1
│  ├─ src/                      includes bridge.cs (section 10)
│  ├─ assets/                   icon, i18n.json, detail_window.json
│  └─ dist/                     (ignored) the installed program:
│     ├─ HardwareMonitor.exe
│     ├─ LhmBridge.exe          built from src\bridge.cs by build.ps1; "Set up" installs it (section 10)
│     ├─ program.json           generated by the build (4.1)
│     └─ data/                  its settings.json, log.txt, power log – yours, never overwritten by an update
├─ discord-plus/                same shape
├─ cube-game/                   same shape
├─ last-bastion/                already its own repo; only gets a program.json
└─ rayman-legends/              program.json + rayman_borderless.ps1 – two files, nothing to build
```

The Toolbox does not care about that folder: every program is added on its own with **Add program…** (4.2), from
wherever it is.

**What ties a program to this repo** is one line in its `Cargo.toml`:
`toolbox-kit = { git = "https://github.com/ItsMaxxx/windows-toolbox" }`. Cargo fetches the library when building
and remembers the exact commit in the program's `Cargo.lock`; `cargo update` moves a program to the newest kit when
*you* decide.

**The common folder earns its keep through one file**, `toolbox-programs\.cargo\config.toml`. Cargo reads such a
file from every folder above the project it builds, so it applies to all program repos at once and belongs to
none of them:

- **One shared build folder.** Without it every repo gets its own `target\` – several GB each, and egui compiled
  again for every program. With it, what one program has compiled the next one reuses.
- **The local kit.** On your PC it redirects `toolbox-kit` from the git address to the `kit\` folder on disk, so a
  change in the kit shows up in every program at the next build, without committing or `cargo update`. Someone who
  clones a single program does not have that file and simply gets the kit from git.

Both are standard Cargo settings; that they combine as described from a parent folder is checked in M0.
**(verify)**

**Versions do not have to match.** Each `.exe` has its kit compiled in and runs on its own. The only meeting point
is the conversation over the pipe (4.3), and that is kept compatible by rule.

Gone compared to today: `__init__.py`, `requirements.txt`, `Setup.exe`, `Uninstall.exe`, `setup\` (6 files),
`runtime\`, all starters, `toolbox\*.py`, the whole `programs\` folder, every `*_gui.py` / `*_settings.py` (one
settings struct in `src\` instead), the separate `i18n.json` of scripts (their few texts move into `program.json`).
`EXPLANATION.md` describes the Python code and is replaced at the end (M6) by a much shorter one.

When started with `cargo run` during development, a program uses the same `dist\data\` as the built one, so there
is one set of settings, not two.

**`dist\` is generated, except `dist\data\`.** That folder is yours. Therefore `build.ps1` replaces only the files
it creates itself and never deletes `dist\` as a whole; a "clean build" clears the shared `target\`, not `dist\`.

---

## 4. How programs are connected

### 4.1 The manifest – `program.json`

Stays JSON (already known, `serde_json` is in use). It gets *smaller*: `page`, `script`, `starter`, `port`, `task`
and `autostart` go away.

```json
{
  "id": "hardware_monitor",
  "name": "Hardware Monitor",
  "version": "2.0",
  "category": "General",
  "description": "CPU and GPU temperature in the taskbar …",
  "exe": "HardwareMonitor.exe",
  "repository": "ItsMaxxx/hardware-monitor",
  "protocol": 1,
  "background": true
}
```

**Written by hand only for scripts and external `.exe`.** A kit program has no `program.json` among its sources:
name, version, description and repository already stand in `[package]` of its `Cargo.toml`, the rest (`id`,
`category`, `background`, texts per language) in `[package.metadata.toolbox]` right below. The kit's build helper
compiles that into the exe and writes `program.json` from it; `build.ps1` puts the file into `dist\`. So the
version exists once, and `protocol` is filled in by the kit that was compiled in.

| Field | Meaning |
|---|---|
| `id`, `name`, `version`, `category`, `description` | as today. A text can be a string or one per language: `"name": { "de": "…", "en": "…" }` – so a script needs no `i18n.json` (its `hint`, button and option labels work the same way) |
| `exe` | what "Start"/"Play" runs – an `.exe` relative to the manifest |
| `run`, `launch`, `options` | as today for scripts (Rayman): a script without window, then a URL/program, with choice buttons |
| `protocol` | the program serves its pipe, and this is the newest protocol version it speaks (4.3). Without it the Toolbox only knows "process running or not". |
| `repository` | `owner/name` on GitHub. With it the Toolbox can update the program – any of the three kinds (section 9). |
| `background` | keeps running without a window → the Toolbox's tray icon carries its menu (section 6); included in "quit/restart all programs" |

Derived from `id`, never written down: pipe name `\\.\pipe\windows-toolbox.<id>`, scheduled task
`WindowsToolbox\<id>`, autostart entry `WindowsToolbox.<id>`, settings file `data\settings.json` in the program's
folder.

### 4.2 Three kinds of programs

| Kind | What you write | What the Toolbox can do | Example |
|---|---|---|---|
| **Script** | `program.json` with `run` + the script | start hidden, show "running", stop, options as buttons | Rayman Legends |
| **External exe** | `program.json` with `exe`, placed **in the other repo** | start, show "running" (by process), open folder | Last Bastion |
| **Kit program** | a crate using `toolbox-kit` | all of the above + its full settings page inside the Toolbox, live status, clean stop, language, install steps | Hardware Monitor, Discord Plus, Cube Game |

Where manifests are found: only in the folders added with the button **Add program…** – one folder, one program
(folder picker; the list is stored in the Toolbox's `data\settings.json`, relative where possible so that moving
everything together keeps working). The Toolbox reads `dist\program.json` in the chosen folder if there is one (a
built program), otherwise `program.json` in the folder itself (a script, or a program unpacked from its release
zip). **Remove** on a program's page takes it off the list again; its folder is not touched.

- **A folder of programs.** If the chosen folder holds no manifest itself, the Toolbox looks one level below it
  and offers every program it finds there – on a new PC one click on `toolbox-programs\` instead of one per
  program.
- **One `id`, one entry.** Two copies of the same program share pipe name and autostart entry, so only one can be
  on the list. Adding the second one asks which of the two to keep.
- **Scripts belong to the agent.** The Toolbox window is a short-lived process, so the agent (section 8) starts
  scripts and puts each one into a named Windows job object (`WindowsToolbox.<id>`). "Running" is then "the job
  has processes" – still true after the window or the agent was restarted – and Stop ends the script together
  with everything it started. **(verify)**

For Last Bastion the agreement in its DESIGN.md 12.7 shrinks to one `program.json` that its `build.ps1` copies
into `dist\`.

### 4.3 Talking to a running program – named pipes instead of ports

Today: TCP on `127.0.0.1` with hand-assigned port numbers that must match in three files. New: one named pipe per
program, name derived from the `id`. One JSON object per line in each direction.

Every kit program gets this for free from the kit. **Nothing is ever answered.** The program sends its current
state whenever it changes; the other side sends commands and keeps the newest state it has seen. There are no
request ids, and a reply can never be confused with an update that happened to arrive at the same moment.

The first message in each direction is `hello`:

- from the Toolbox: `{ "protocol": 1, "language": "de", "theme": "dark", "zoom": 1.0, "want": ["status", "page"] }`.
  `want` names the states this connection is interested in: the agent wants `status` and `tray`, the window
  `status` and `page` of the program it is showing.
- from the program: `{ "protocol": 1, "id": …, "version": …, "pid": … }`.

**States** – sent by the program: once after `hello`, then again on every change.

| State | Content |
|---|---|
| `status` | `{ "running": true, "state": "ok", "text": "CPU 52 °C · GPU 47 °C" }` – `running`: are the workers switched on (roles, section 5); `text` is what the Toolbox shows under the name |
| `page` | the program's settings page as data: sections, building blocks, current values (4.4) |
| `tray` | what the program wants in the tray, as data: a status line, its menu entries, optionally small live displays (section 6) |
| `toast` | a one-off message for the bottom bar, e.g. why a `set` or an `action` was refused |
| `bye` | the program is about to end on purpose. A pipe that closes without it means: crashed (section 8). |

**Commands** – sent to the program:

| Command | Effect |
|---|---|
| `set` | `{ "key": "icon_interval", "value": 1 }` – change one setting. The program applies it **at once** and saves it. The changed page arrives as a state, like every other change. |
| `action` | `{ "name": "connect" }` – a button on the page was pressed |
| `start`, `stop` | switch the workers on or off (section 5) |
| `show` | open the program's own window (the Hardware Monitor's detail window) and bring it to the front |
| `hello` again | language, theme or zoom were changed in the Toolbox; the program switches and remembers them |
| `quit` | end cleanly; the caller waits on the process handle |

**Sliders.** While a slider is being dragged, the Toolbox sends `set` for every step, shows its own value and
ignores pushed values for that key until the mouse button is released – otherwise the handle would jump. The
program applies each value at once but writes the file with a short delay (and when it ends), not per step.

The pipe is also the **single-instance lock**: if it already exists, the second start sends `start` and `show`
and exits.

Because a program in its own repo may be built with an older or newer kit than the Toolbox, the first message of a
connection carries a **protocol version**, and both sides ignore fields they do not know. A building block the
Toolbox does not know yet is shown as a line "needs a newer Toolbox" instead of breaking the page. The protocol
only ever grows: existing commands and fields are never given a new meaning, so a new Toolbox understands every
old program.

`hello` carries the **Toolbox's language and look** (light/dark, zoom). A program remembers them, so the window it
opens itself – the detail window, its own settings window – looks like the Toolbox. A program that was not running
while they were changed picks them up the next time the Toolbox connects.

No program of ours runs with admin rights (section 10), so Toolbox and programs talk as equals: the pipe accepts
the logged-in user and nobody else – stricter than today's open TCP port.

### 4.4 Where a program's settings UI lives

**The program defines its page, the Toolbox displays it.** Everything stays in the one Toolbox window, as today –
but no code of the program runs inside the Toolbox.

How: a page is *described*, exactly as today's pages are with the building blocks of `base.py` (Section, Text,
Choice, Switch, Slider, Combo, Entry, Buttons). In Rust the description is plain data that can be sent over the
pipe:

```
 program (HardwareMonitor.exe)                              Toolbox window
 ─────────────────────────────                              ──────────────
 fn page(p, settings) { … }  ──►  page as JSON  ──pipe──►   kit draws it with egui
                                                            user moves a slider
 apply now + save            ◄──  set key value ◄─pipe───   
 page again (new values)     ──────────────────────────►    redraw
```

- **Instant**: `set` reaches the running program directly – no file to notice, no "reload". Like Rayman's
  left/centre/right today, for every setting of every program.
- **Live**: the program pushes the page again when a status line changes; the Toolbox only listens while that page
  is visible.
- **Program not running**: the Toolbox starts its `.exe` with the workers switched off – no tray entry, only "serve
  my page and save changes" – and it ends when the Toolbox leaves the page. So settings can be changed for a
  stopped program too, and the code that draws up the page is still only in the program. **Start** on that page
  sends `start` to this very process instead of launching a second one (section 5).
- **Without the Toolbox**: the program's tray menu has "Settings". If a Toolbox is there, it opens it on this page
  (today's ⚙ in the detail window does that already). If not, the program shows the same page in a small window of
  its own – the kit function that draws a page in the Toolbox draws it there as well. One description, one
  renderer, two places.
- **Scripts** (Rayman) keep `options` in the manifest; the Toolbox turns them into the same page data itself and
  writes the file the script reads, as today.

Around the program's page the Toolbox adds what it knows itself, the same for every program: name, description,
running/stopped/crashed, Start · Stop · Restart, "Start with Windows", "Restart after a crash" (section 8), Open
folder, README, version, Set up / Uninstall (section 9), Update, Remove from the list.

### 4.5 When building blocks are not enough

A described page can only show what the kit has a block for. Three ways past that limit, from cheap to powerful –
they build on each other and none has to exist on day one:

| Step | What | Good for | Cost |
|---|---|---|---|
| **1. More blocks** | add to the kit as needed: tabs, table, value list, graph, progress bar, image, list editor | almost everything, including most of the Hardware Monitor's detail window (tabs of value tables) | one block = a struct + a draw function, written once, usable by all programs |
| **2. Canvas block** | the program sends drawing commands (lines, rectangles, text, colours) and receives mouse events for a rectangle on the page | one-off custom widgets without teaching the Toolbox anything: the fan-curve editor, the power chart | written once in the kit; the program then draws freely, but through messages (fine for a chart, not for a game) |
| **3. Embedded window** | a block that reserves an area; the program's own egui window is placed *inside* the Toolbox window there (a Win32 child window) | anything egui can do, at full speed | fiddly: focus, resizing, DPI. **(verify)** before relying on it |

Plan: start with the blocks today's pages use, add step-1 blocks whenever a page needs one, build the canvas when
the fan curves are ported (M4). Until then – and for anything that fits none of these – a program opens **its own
window**, as the Hardware Monitor's detail window does today. So the limit never blocks a feature; it only decides
whether something appears inside the Toolbox or beside it.

Considered and rejected:

- *Compiling each program's page into the Toolbox* – no limits at all, but the Toolbox would have to be rebuilt for
  every program and a program in its own repo could not take part. That is today's coupling again.
- *A plug-in DLL per program that draws with egui directly* – Rust guarantees no stable interface between separately
  built files; Toolbox and program would need the exact same compiler and egui version or crash.
- *Pages as HTML in an embedded browser* – unlimited, but a browser engine in the Toolbox, a second look, and
  nothing of the egui work could be shared.

---

## 5. `toolbox-kit` – the shared library

One crate, modules by task. This is where today's `base.py`, `storage.py`, `i18n.py`, `winutil.py`, `starter.cs`,
`make_exe.py` and most of `setup.ps1` end up – once.

| Module | Replaces | What it does |
|---|---|---|
| `app` | boilerplate in every `*.py` | `kit::run(App { … })`: decides the role (background/window), single instance, pipe server, logging, panic → log file |
| `settings` | `storage.py`, `load_settings()` checks, `defaults` in pages | `Settings<T>`: load/save a `#[derive(Serialize, Deserialize, Default)]` struct with `#[serde(default)]`; unknown keys are kept; invalid values fall back per field |
| `page` | `base.py` (the building blocks) | describe a page as data: sections and building blocks bound to settings fields; turn `set`/`action` back into changes (4.4) |
| `ui` | `base.py` (the drawing) | theme (section 7); draws a described page with egui – used by the Toolbox and by a program's own settings window; toast, link bar |
| `i18n` | `i18n.py` | same `i18n.json` format as today; `t!("page.status")`. Release builds compile the file in (`include_str!`); debug builds read it from `assets\` at start, so a text can be changed without compiling |
| `ipc` | socket code in 4 files | pipe server/client, the standard commands |
| `tray` | pystray | describe status line, menu and live displays; hand them to the Toolbox – or show an own icon when there is none (section 6); icons drawn from text (temperature numbers, via GDI) |
| `autostart` | `winutil.py` (PowerShell) | Run key – direct API calls, name derived from `id` |
| `process` | `winutil.py`, starters | start hidden, run a helper elevated through its scheduled task (no prompt), wait for exit, job objects for scripts, efficiency mode (EcoQoS) |
| `update` | `git pull` + Setup | one function for every kind of program and the Toolbox itself: look for a release, download, check, swap files (section 9) |
| `setup` | `setup.ps1`, `"downloads"` | idempotent steps a program declares: download + SHA-256 + unzip, winget package, copy into the protected folder, task (section 9) |
| `manifest` | `find_manifests()` | read `program.json` (the Toolbox); build helper that generates it from `Cargo.toml` and embeds it (programs, 4.1) |

### What a kit program looks like

```rust
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    icon_interval: f32,      // seconds
    close_to_tray: bool,     // a switch: off unless switched on (opt-in)
}
impl Default for Settings {
    fn default() -> Self { Self { icon_interval: 2.0, close_to_tray: false } }
}

fn page(p: &mut kit::Page, s: &Settings) {
    p.section(t!("page.refresh"), |p| {
        p.text(t!("page.refresh.text"));
        p.choice(t!("page.refresh.icons"), "icon_interval", &[(0.5, "0.5 s"), (1.0, "1 s"), (2.0, "2 s")]);
        p.switch(t!("page.close_to_tray"), "close_to_tray");
        p.buttons(|b| b.button(t!("page.open_log"), "open_log"));
    });
}
```

**A new setting = one field + one line in the page.** Defaults, validation, loading, saving, sending the page and
applying `set` are the kit's job. The four places from section 1 become one file. It is deliberately the shape of
today's `layout()` – the existing pages translate block by block.

### The roles of an exe

One file, several ways it runs – so that the part that runs all day stays tiny:

- **background role**: pipe server, settings, and the workers with their tray data. Plain Win32 message loop.
  Never initialises egui or a graphics context. The workers can be **on** (the program "runs") or **off** (it
  only serves its settings page to the Toolbox, 4.4) – the same process either way, switched with `start` and
  `stop`. With the workers off it ends as soon as nobody is connected any more. That way exactly one process holds
  the pipe and writes `settings.json`, and Start can never mistake an open settings page for a running program.
- **window role**: an egui window (the detail window; the settings page when there is no Toolbox). A second process
  of the same `.exe`, started by the background role on a click or on `show`; it ends when the window closes and
  gives all its memory back.

Double-clicking a program always means: *make sure it runs, then show its window.* Programs without a background
part (Cube Game) simply have no background role.

Why two processes instead of one hidden window: a hidden egui window keeps its renderer in memory, and egui's event
loop is known to be awkward to wake from tray events while its window is hidden. Two roles avoid both by
construction. **(verify in M0** – if a single process that creates and destroys its window on demand turns out
clean and equally cheap, it is the simpler choice and the kit hides the difference anyway.)

---

## 6. Tray icons

**One rule for all: the Toolbox owns the tray. Programs have no icon of their own; they tell the Toolbox what to
show** (`tray` in 4.3) – the same idea as with pages.

Left click on the Toolbox icon opens the window. Its menu, one level of submenus and no deeper:

```
Open Toolbox
──────────────────────────────────────
● Hardware Monitor   CPU 52 °C · GPU 47 °C   ▸  Open · Settings · Refresh rate ▸ · Restart · Stop
● Discord Plus       In a call               ▸  Settings · Connect · Restart · Stop
○ Cube Game                                  ▸  Play
○ Last Bastion                               ▸  Play
──────────────────────────────────────
Quit ▸  Only Toolbox · Toolbox and all programs
```

The status line and the entries behind `▸` come from the program; Open/Restart/Stop/Play the Toolbox adds itself.
The same status lines are the icon's tooltip, so hovering shows the temperatures without a click.

**The temperatures.** Today the Hardware Monitor's two icons *are* its display: numbers you read without touching
the mouse. Under a single Toolbox icon they are one hover or one click away instead. For that reason a program can
also declare **live displays** – a short text and a colour, e.g. `52` in green – and the Toolbox can show them as
additional small icons next to its own. They are still the Toolbox's icons (same click, same menu), not the
program's. One switch on the Hardware Monitor page, "Temperatures in the taskbar", turns them on. Default: **off**
(opt-in, section 2).

**Without the Toolbox** a background program must still be reachable (how else to quit it?). Rule in `kit::tray`:
as long as no Toolbox agent is connected, the program shows the same data in an icon of its own; the moment the
agent connects, that icon disappears. No polling: the agent connects to every running program when it starts, a
program connects to the agent when it starts, and a closed pipe tells either side that the other one is gone.

**Windows 11 hides new tray icons** in the overflow (`^`) until the user drags them out or switches them on under
Settings → Personalisation → Taskbar → Other system tray icons. That goes for the Toolbox icon and for every live
display. Two consequences:

- The steps are described where they are needed: on the Toolbox's settings page and right next to the switch
  "Temperatures in the taskbar", with a button that opens that Windows settings page.
- Windows remembers the choice per icon. Each icon therefore gets a fixed identity (Toolbox icon, live display 1,
  2, …) that never changes. Whether the choice survives an update (the exe is renamed aside and replaced) and a
  moved folder is checked in M0 **(verify)**; if moving loses it, the page says so.

Cost while idle: a running program pushes `tray` only when its line changes (the Hardware Monitor at its icon
rate, Discord Plus a few times a day); the agent wakes for that message and sleeps again.

---

## 7. Look and feel

egui through **eframe** (the standard way to run egui as a normal Windows program; Last Bastion's editor uses egui
through macroquad instead, which redraws every frame – right for a game, wrong for a tool).

- **Reactive drawing**: egui only redraws on input or when asked (`request_repaint_after`). A page with live values
  asks for a redraw at its refresh rate while visible; everything else draws nothing while idle.
- **Renderer**: `glow` (OpenGL) rather than `wgpu` – smaller exe, faster start, less memory for a few panels.
  **(verify)**
- **Theme** in `kit::ui::theme(ctx)`, applied by every window – the editor's `style()` carried over: egui's dark
  visuals, text sizes 12 / 15 / 15 / 19 / 14 (small / body / button / heading / monospace). Colours for good / bad /
  warn and the green accent from today's `base.py` come along as constants. Light/dark/system and the zoom are
  settings of the Toolbox; programs receive them in `hello` (4.3) and use them for their own windows.
- **Layout** of the Toolbox window is what the editor already does: `TopBottomPanel` (category tabs, gear),
  `SidePanel` (programs of the tab), `CentralPanel` (the page), bottom bar (link target on hover, toasts).
- Solved along the way: the Tk scroll stripes (egui scrolls by redrawing, there is nothing to smear), zoom
  (`Ctrl` + wheel is built in), DPI scaling, tooltips. The planned PySide6 port is no longer needed.
- Check early: egui's built-in font must have the symbols used today (`●`, `⚠`, `⚒`, `⚙`); otherwise ship one small
  symbol font in the kit. **(verify)**

---

## 8. The Toolbox itself

`Toolbox.exe`, two roles as in section 5.

**Agent (background)** – started by the first double-click, or with Windows if that is switched on (Run key):
the one tray icon with every program's menu and status (section 6), pipe `windows-toolbox.toolbox`
(`show [id]`, `quit`, starting and stopping scripts – 4.2). No timers – it sleeps until a click or a message from
a program arrives.

**Crashes.** The agent is connected to every running kit program anyway. If such a pipe closes without a `bye`
(4.3), the program did not end on purpose: its entry shows "crashed" with a link to its `log.txt` (the kit writes
a panic there), in the menu and on its page. "Restart after a crash" is a switch per background program, off by
default; switched on, the agent starts the program again – at most three times in ten minutes, then it gives up
and says so.

**Window** – opened by icon click, double-click on `Toolbox.exe`, or `show`:

- library: tabs per category, programs on the left; on the right the program's page (4.4) inside the Toolbox's
  frame (status, start/stop, autostart, folder)
- only the visible page is connected to its program, and only while the window is visible
- gear: appearance and language (sent to every running kit program with `hello`), programs (**Add program…**,
  remove from the list), start with Windows, making the tray icon visible (section 6), **export / import all
  settings** (one zip with the `data\settings.json` of the Toolbox and of every program on the list),
  quit/restart, update, clean up, uninstall (section 9)

Start-up cost today: Python interpreter + customtkinter + importing every program's page. New: one native exe that
reads a handful of small JSON files. Expectation: window visible in a fraction of a second; measured in M0 against
today's Toolbox instead of promised here.

---

## 9. Install, update, uninstall

### Install

Today: clone → `Setup.exe` → Python via winget → copy 108 MB → pip → compile starters → downloads → admin steps.

New: **download the release zip, unpack, double-click `Toolbox.exe`.** There is no setup program. Nothing to copy,
nothing to compile, no Python. A program is installed the same way: unpack its release zip (the content of its
`dist\`) anywhere, then **Add program…**.

**A hint where the download is.** The `.exe` files are not signed (signing costs money). Windows SmartScreen can
therefore warn once at the first start of a downloaded program ("Windows protected your PC" → *More info → Run
anyway*), and a virus scanner may look twice at an unsigned program that downloads updates. The release page of
every repo and the top of its README say so in two lines, so the warning is expected instead of alarming. Programs
the Toolbox downloads itself (updates) and programs you build yourself are not affected.

What remains are the things a *program* needs from Windows, and they belong to that program:

```rust
// hardware-monitor – declared once, runs from "Set up" on its page (in the Toolbox or in its own window)
kit::setup::steps![
    Winget   { id: "namazso.PawnIO" },                                  // needs admin
    Download { name: "LibreHardwareMonitor", version: "0.9.6", url: "…", sha256: "…", into: Protected("lhm") },
    Copy     { file: "LhmBridge.exe", into: Protected("") },            // needs admin
    Task     { run: Protected("LhmBridge.exe"), elevated: true },       // needs admin; started on demand, not at logon
]
```

`Protected(…)` is the folder `C:\Program Files\WindowsToolbox\<id>\`, which only admins can write to. Everything
that later runs with admin rights without a prompt lies there and nowhere else (section 10 says why). The
checksum of a download into it is checked on the copy inside the protected folder, by the elevated helper.

Each step checks first whether it is already done (as `setup.ps1` does today). If any pending step needs admin
rights, Windows asks **once** and those steps run in a short-lived elevated helper; the program itself stays
without admin rights. "Set up" in the Toolbox just asks the program to do this and shows its progress; a program
started alone with missing requirements shows the same button itself. Programs that need nothing (Cube Game,
Discord Plus, scripts) have no such button.

`Setup.exe`, `Uninstall.exe`, `setup\`, `runtime\`, `requirements.txt`, all starters: gone.

### Building (developer PC)

Each repo has the same small `build.ps1`: `cargo build --release` → copies the exe and the generated
`program.json` into its `dist\` (the running-exe rename trick from last-bastion's `build.ps1`; it never deletes
`dist\`, section 3). Requirement: Rust, already installed.

**A release** is made by the second file, `release.ps1`: it runs `build.ps1`, zips `dist\` without `data\`, and
publishes the zip together with its SHA-256 as GitHub release `v<version>` (`gh release create`), the version
taken from `Cargo.toml`. It refuses if that version was already released or the repo has uncommitted changes.
Name, icon and description in Task Manager come from a resource compiled into each exe by a build script – that is
all `make_exe.py` and `starter.cs` were for.

### Update

Today `git pull` + `Setup.exe -Restart`. With compiled programs a user PC has no source, so:

- **User PC**: on a click on "Check for updates" (automatic checking is a switch, off by default) the kit asks
  GitHub for the latest release of *that* repo (the manifest names it:
  `"repository": "ItsMaxxx/hardware-monitor"`), downloads the zip, checks its SHA-256, swaps the files (a running
  exe is renamed aside, `data\` is never touched), restarts what was running.
- **Developer PC** (the program sits in a `dist\` below a git repo): the update button is replaced by the hint
  "run `build.ps1`".

An update is the same for every kind of program – load the zip, replace everything except `data\`, start again –
so it is **one function, `kit::update`, for all three kinds** and for the Toolbox itself: a script or an external
`.exe` only needs `repository` in its manifest. The Toolbox calls it for the programs on its list ("Update" on a
program's page, "Update all" in its settings); a kit program calls the same function itself when it runs without
a Toolbox. Every program is updated on its own, each with its own version.

If an update brings a new bridge or a new LibreHardwareMonitor version, the copy in the protected folder is older
than the one the program expects: its page shows "Set up" again, and that asks for admin rights once.

What the checksum is worth: the SHA-256 of an update comes from the same release as the zip, so it only catches a
**broken** download, not a manipulated one – the trust lies in HTTPS and the GitHub account. The checksum of
LibreHardwareMonitor is different: it is pinned in the program's code and does catch a swapped file.

Later, if wanted: a list of known programs in the Toolbox ("Get programs") that downloads and adds one with a
click – the same release zips, just without the manual unpack.

### Uninstall

Per program: **Uninstall** on its page – the program undoes its own steps (task, protected folder, autostart);
then its folder can be deleted. Gear → Uninstall does that for all programs and removes the Toolbox's Run key and
shortcuts. PawnIO stays, as today.

**Clean up.** With a portable program the natural way to get rid of it is to delete its folder – which leaves a
task, a Run key or a protected folder behind. Because every such name starts with `WindowsToolbox`, the Toolbox
can find them: Gear → Clean up lists the entries whose program no longer exists and removes them after a click
(one admin prompt if a task or a protected folder is among them). Nothing is removed without being listed first.

---

## 10. The programs

| Program | Today | Port | Notes |
|---|---|---|---|
| **Rayman Legends** | manifest + `.ps1` | none | works unchanged with the new script page |
| **Last Bastion** | page in this repo | manifest moves to its repo | nothing of it remains here |
| **Discord Plus** | Python, pystray, TCP | small | Discord's own pipe (`\\.\pipe\discord-ipc-0`) is a few lines in Rust; the login needs one small HTTPS client crate. Good first real kit program. |
| **Hardware Monitor** | Python, pythonnet → LibreHardwareMonitor (.NET) | large | see below |
| **Cube Game** | Python, pygame-ce | largest | → macroquad, the engine already known from Last Bastion. Last, because it is the only reason Python stays installed. |

### Hardware Monitor – the one hard question

**LibreHardwareMonitor stays exactly what it is**: the developers' unmodified `LibreHardwareMonitorLib.dll`,
downloaded in a pinned version with checksum, and updated by changing version + URL + SHA-256 in one place – as
today. PawnIO likewise. Only our own code (today ~240 KB of Python around it) is rewritten.

The question is only how Rust *calls* a .NET library – today pythonnet does that:

| | How | For | Against |
|---|---|---|---|
| **A. Bridge process** (recommended) | a tiny C# program `LhmBridge.exe` (one file, ~150 lines) loads the DLL and passes values on as JSON lines over a pipe: `open`, `read`, `set_fan` | LHM untouched and isolated; a new LHM version needs no change on our side as long as its sensor names stay | the one piece of own code that is not Rust; one more process with the .NET runtime in memory (pythonnet loads the same runtime today); needs a C# build (the `csc.exe` shipped with Windows compiles today's starters – whether it is enough for the bridge: **verify**) |
| B. Host .NET inside the Rust process | load the .NET runtime through its hosting API and call LHM from Rust | 100 % Rust, one process | fragile, lots of `unsafe`, hard to debug |

Plan: **A.** The Nvidia temperature already comes straight from `nvml.dll` (part of the driver) today; that
shortcut moves to Rust unchanged.

### Admin rights: only the bridge

Today the whole monitor runs with admin rights (CPU and mainboard sensors need them) and takes commands from the
Toolbox – any program of your user could send them too. New:

```
 Toolbox ◄──pipe──► HardwareMonitor.exe ◄──pipe──► LhmBridge.exe ──► LibreHardwareMonitorLib.dll ──► PawnIO
 (normal)           (normal: tray data, page,       (admin: open, read, set_fan – nothing else)
                     detail window, logs, curves)
```

- `HardwareMonitor.exe` runs like every other program, without admin rights. Its windows, its page and an
  embedded window (4.5) need no special handling.
- `LhmBridge.exe` is the only thing with admin rights: ~150 lines that pass sensor values on and set a fan speed.
  It has no settings, no files of its own, no window.
- The monitor starts the bridge through a **scheduled task** that "Set up" registers once (one admin prompt, as
  today). The task has no trigger of its own – it only exists so that the start needs no prompt. Whether the
  monitor starts with Windows is its ordinary, opt-in autostart.
- **The task must not be a back door.** It runs a file with admin rights and without asking. If that file lay in
  `dist\`, where every program of your user may write, swapping `LhmBridge.exe` or the DLL it loads would hand out
  admin rights silently. So "Set up" copies the bridge and LibreHardwareMonitor into
  `C:\Program Files\WindowsToolbox\hardware_monitor\`, which only admins can write to, and the task points there.
  The price: this one part is not portable (it does not need to be – the task no longer depends on where the
  program's folder lies), and a new bridge or LHM version asks for admin rights once.
- Because a task cannot hand over stdin/stdout, monitor and bridge talk over a named pipe that admits the
  logged-in user. The bridge ends when that connection closes. It does **not** try to find out who is calling:
  any check would rest on files the user can replace. The protection is what the bridge can do at all – read
  sensors, set a fan speed within the limits the mainboard allows – and that is deliberately everything a program
  of your user can reach through it. **(verify:** pipe from a normal to an elevated process**)**
- Without the task (not set up, or started on a PC without admin rights) the monitor still runs and shows what
  needs no admin rights – as today's double-click on `HardwareMonitor.exe` does.

Things to keep from today's monitor: fast start (CPU/GPU first, rest later), reading only what the open tab shows,
efficiency mode, power log, fan curves, drive health, privacy mode. The detail window's *content* stays editable
without compiling: `detail_window_content.py` becomes a data file (`detail_window.json`) that is read each time the
window opens – same "save, reopen, see it" loop as now.

---

## 11. Migration – always a working Toolbox

Each milestone ends in a state that can be used daily and committed.

The Python code stays where it is (`toolbox\*.py`, `programs\<name>\`) until its Rust successor works. A program
leaves this repo at the moment it is ported: the new version starts in its own repo, the old folder here is
deleted in the same step. The git history of the Python version stays in this repo.

**M0 – Prototype (throw-away, a few days).** Answers every *(verify)* and tries the riskiest self-built part once
end to end:

- tray agent + egui window as two roles of one exe; idle CPU and memory of both; start time next to today's Toolbox
- glow vs wgpu; symbols in the font
- **a mini page through the whole chain**: a second exe describes a page with one switch, one slider and one
  status line; the first one draws it, sends `set`, shows the pushed update. The page system is a small UI
  framework of our own and the part whose effort is hardest to guess – this shows what it really costs before
  everything else rests on it.
- the bridge: `csc.exe` builds a minimal one; copied into the protected folder; started from there by a scheduled
  task; pipe from a normal process to it
- `.cargo\config.toml` in a parent folder: shared build folder and local kit for two repos below it
- tray icons under Windows 11: fixed identities; does "show this icon" survive an update and a moved folder
- a script in a named job object: found again after the agent was restarted, stopped as a whole
- the slider rule (4.3) on the mini page: no jumping handle, one write to the file per drag

Result, written into this file: measured numbers, corrections where an assumption was wrong, and **the formats**
(section 14) – so that M1 starts from facts and fixed formats instead of sketches.

**M1 – Kit + new Toolbox, next to the old one.** `kit\` (`app`, `settings`, `ui`, `i18n`, `ipc`, `tray`,
`manifest`, `page`) and `toolbox\src\` in this repo, `Toolbox.exe` with library, page display, script page, tray
menu, **Add program…**. The Python programs are added from their current folders and appear as
they are: started through their existing starters, status through their existing ports (the kit's IPC client
understands the old `"port"` field until the last Python program is gone). Their "Settings" button opens the **old**
Toolbox on that page – the `show <id>` command it already has. The old Toolbox is kept as `ToolboxClassic.exe`.
The Python programs keep their own tray icons until they are ported; the single icon fills up program by program.

**M2 – Rayman + Last Bastion.** Rayman moves into its own folder (two files) and proves the script page;
Last Bastion gets its `program.json`. First two programs outside this repo that no longer need the classic
Toolbox.

**M3 – Discord Plus in Rust.** First kit program and first one built in a repo of its own against the kit's git
address: background role, its page shown in the new Toolbox (running and stopped), instant `set`, autostart, an
action button (Connect), its menu under the Toolbox icon and the fallback icon without it, its own release zip
and update. `template\` in this repo is extracted from it.

**M4 – Hardware Monitor in Rust.** Bridge as the only elevated part, status line and live displays in the tray,
detail window (with the new blocks and the canvas from 4.5), settings, setup steps (PawnIO, LHM, task).
After this, `ToolboxClassic.exe` has no page left to show and is deleted together with `toolbox\*.py`.

**M5 – Cube Game in Rust; Python leaves.** Then `runtime\`, `requirements.txt`, `setup\`, `Setup.exe`,
`Uninstall.exe` are deleted and the release zip + updater (section 9) become the only way to install.

**M6 – Wrap-up.** README/EXPLANATION rewritten for the new structure; whatever daily use shows to be missing.

Settings files are carried over once per program: on first start the Rust program reads the old
`data\<id>.json` if its own `data\settings.json` does not exist yet.

---

## 12. Before / after

| Task | Today | After |
|---|---|---|
| Add a small script | `program.json` + script + `i18n.json`, then Install in the Toolbox | `program.json` + script – two files, no Install click |
| Add an existing `.exe` / other repo | write a Python page in this repo | one `program.json` in that folder, **Add program…** |
| Add a program with settings | folder, manifest, page class, starter fields, `Program(is_running, stop, start)`, own IPC port and socket code, README, run Setup | copy `template\` into a new folder, rename, write the settings struct and page, **Add program…** |
| Add a setting | 4 places | 1 field + 1 line |
| Change a port / task name | 3 files | does not exist |
| Add a Python package | `requirements.txt` + check line in `setup.ps1` + Setup | `cargo add` |
| Install on a new PC | clone, Setup.exe, minutes | unzip, double-click |
| Files after install | 3273 in `runtime\` alone | one exe per program |

### What gets worse

- **Compiling.** A changed line needs a build (seconds with incremental builds, as in Last Bastion) instead of a
  restart. Content that is edited often stays in data files for that reason (detail window, i18n during development).
- **Pages are limited to the kit's building blocks** (as they are to `base.py`'s today); 4.5 describes the ways
  past that.
- **Temperatures are no longer always visible** with a single tray icon, unless the live displays are switched on.
- **The Hardware Monitor port is real work** and carries a technical risk (calling the .NET library through an
  elevated bridge). The other one is the page system (M0).
- **Unsigned downloads**: Windows may warn at the first start of a downloaded program (section 9).
- **EXPLANATION.md** (125 KB, about the Python code) becomes obsolete and has to be rewritten at the end.

### Open decisions

None at the moment.

Settled: everything optional is opt-in, so "Temperatures in the taskbar" is off by default; release zip instead
of clone + setup (the project goal becomes "unzip = runs", for you "clone + `build.ps1` = runs"); settings per
program folder, with a button "Export all settings" in the Toolbox as the replacement for copying one `data\`;
the C# bridge for LibreHardwareMonitor, and only it with admin rights, installed in a folder only admins can
write to; "Restart after a crash" exists as a switch, off by default.

---

## 13. Testing without windows

Nothing may pop up while testing (someone may be playing on this PC). The design makes that easy, because the
parts that matter are data:

- **Pages and tray menus** are described as data before anything is drawn. A test builds the page for given
  settings and checks the description – which blocks, which values, which entries – with no window at all.
- **Page keys**: every key a page names (`"icon_interval"`) must be a field of the settings struct – a typo would
  otherwise be a switch that does nothing. The kit has one test function for that; it builds the page and looks
  each key up. Every program calls it in one line, and `template\` already contains that line.
- **The conversation** (`status`, `page`, `set`, `action`, `tray`) is tested by starting a program with its
  workers off and talking to its pipe: send `set`, read the pushed page, look at the saved file.
- **Settings**: load a file with missing, unknown and invalid fields; check what comes out.
- **Texts**: every id used in the code exists in `i18n.json` in every language (one test in the kit, run for
  every program).
- **Setup steps**: each step's "already done?" check runs without changing anything.
- **The drawing itself** is the small rest. egui can draw into memory instead of a window; whether that is worth
  using is decided when the first drawing bug shows up, not before.

All of it runs with `cargo test`, without admin rights and without a window.

---

## 14. Working from this document

This file fixes the direction. Before it is enough to build from without deciding things on the way, three gaps
have to be closed – in this order:

1. **M0 writes down the formats.** Today they are only sketched in examples. After M0 this file contains, as
   fixed lists: every building block with its fields; the exact JSON of `hello`, of every state (`status`, `page`,
   `tray`, `toast`, `bye`) and of every command; the fields of `program.json` and of `[package.metadata.toolbox]`;
   the public functions of each kit module.
   Two sessions must not be able to invent two different formats.
2. **M0 replaces every *(verify)*** by what was measured or found, and names the crates and versions that were
   actually used (looked up at that time, not taken from memory).
3. **Every port starts with a checklist of what the old program can do**, written from its Python code and its
   README before the first line of Rust: every setting, every menu entry, every command, every special case
   (for the Hardware Monitor e.g. fast start, privacy mode, power log, fan curves, task repair, efficiency mode).
   The list lives in the new repo's README and is ticked off; the port is done when nothing is left unticked or
   consciously dropped. The old code is the specification – this document only names the features in passing.

Rules that hold for all work on this project:

- **Nothing pops up** while building or testing: no windows, no consoles (section 13).
- **User functions are files and buttons**, never command-line switches (section 2).
- **Instructions belong on the program's page**, where they are needed – not only in a README.
- **Opt-in**: a new switch is off by default; do not ask what the default should be (section 2).
- **One fact, one place**: if a value has to be written twice, derive it or move it into the kit.
- **Ask before deciding** something this document leaves open, with a recommendation; then write the decision
  into this file so it is not asked again.
- **Keep this file true**: when the build shows that something here is wrong, the file is corrected in the same
  commit.
- Code, comments and documents in English; each milestone ends usable and committed.
