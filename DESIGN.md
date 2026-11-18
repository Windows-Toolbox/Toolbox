# Windows Toolbox in Rust – design

Status: **M1 (kit + new Toolbox next to the old one) built** (section 11); its hand test and M2 are next. It describes where the Toolbox
should end up, why, and in which order to get there without ever having a broken Toolbox in between. The fixed
formats are in section 15.

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

Decided after M0 (2026-11-16):

- **Renderer: glow (OpenGL) only.** On a PC without usable OpenGL the window shows a clear message instead of
  staying empty; no wgpu fallback (it would cost ~6 MB in every exe with a window) (section 7).
- **All repos live in the GitHub organization `Windows-Toolbox`** (display name "Windows Toolbox"), not under the
  personal account: one link shows every program, the personal profile stays tidy. Names of folders, repos and
  exes: table in section 3.

M0 (a throw-away prototype) confirmed or corrected every assumption that was marked **(verify)**; the one left in
4.5 (the embedded window) belongs to a later step and is checked before that step relies on it.

This file says what, why, in which order; section 15 holds the fixed formats to build from, section 14 how to work
from both.

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
Toolbox/                        this repo (GitHub: Windows-Toolbox/Toolbox)
├─ README.md
├─ DESIGN.md
├─ Cargo.toml                   lists the crates below so one command builds them (Cargo.lock: pinned versions, written by cargo)
├─ .cargo/config.toml           the Visual C++ runtime is linked in (section 9)
├─ build.ps1                    release build → dist\ (the trick from Last Bastion: a running exe is renamed aside)
├─ release.ps1                  build + zip + GitHub release (section 9; comes with M3)
├─ kit/                         toolbox-kit: what programs and the Toolbox share (section 5)
├─ toolbox/                     the Toolbox → Toolbox.exe   (src\, assets\: icon and i18n.json, compiled in)
├─ demo/                        Kit Demo: the smallest kit program – the kit's tests talk to it (section 13)
├─ template/                    copy this folder to start a new program (comes with M3)
└─ dist/                        (ignored) Toolbox.exe + data\ with its settings.json and log.txt
```

The programs lie wherever you like. Practical is one folder for all of them – on this PC
`E:\Software\Windows Toolbox\` – with one repo per program below it:

```
Windows Toolbox/                just a folder, not a repo
├─ .cargo/config.toml           one small file for all repos below: shared build folder, local kit (see below)
├─ target/                      the shared build folder (created by cargo)
├─ Toolbox/                     this repo (above) – placed here too, so it uses the shared build folder
├─ Hardware Monitor/            its own git repo
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
├─ Discord Plus/                same shape
├─ Cube Game/                   same shape
├─ Last Bastion/                already its own repo; only gets a program.json
├─ Rayman Legends/              program.json + rayman_borderless.ps1 – two files, nothing to build (+ .gitignore for data\)
├─ Screen Zones/                same shape as Hardware Monitor – the successor of the Rayman script (section 11)
└─ Steam Shortcuts/             same shape – games into Steam, desktop icons onto Steam (section 11)
```

**Names** (decided 2026-11-16): the folder carries the program's name; the GitHub repo the same with `-` for the
space (GitHub allows no spaces); the exe has no space (one program starting another never trips over quoting –
Task Manager and taskbar still show the name with space, from the compiled-in resource); the `id` stays lower case
with `_`, so pipe, task and settings of the old and the new version meet.

| Program | Folder | GitHub | Exe | `id` |
|---|---|---|---|---|
| Toolbox | `Toolbox` | `Windows-Toolbox/Toolbox` | `Toolbox.exe` | `toolbox` |
| Hardware Monitor | `Hardware Monitor` | `Windows-Toolbox/Hardware-Monitor` | `HardwareMonitor.exe` | `hardware_monitor` |
| Discord Plus | `Discord Plus` | `Windows-Toolbox/Discord-Plus` | `DiscordPlus.exe` | `discord_plus` |
| Cube Game | `Cube Game` | `Windows-Toolbox/Cube-Game` | `CubeGame.exe` | `cube_game` |
| Last Bastion | `Last Bastion` | `Windows-Toolbox/Last-Bastion` | `LastBastion.exe` | `last_bastion` |
| Rayman Legends | `Rayman Legends` | `Windows-Toolbox/Rayman-Legends` | – (script) | `rayman_legends` |
| Screen Zones | `Screen Zones` | `Windows-Toolbox/Screen-Zones` | `ScreenZones.exe` | `screen_zones` |
| Steam Shortcuts | `Steam Shortcuts` | `Windows-Toolbox/Steam-Shortcuts` | `SteamShortcuts.exe` | `steam_shortcuts` |
| the kit | `Toolbox\kit` | inside `Toolbox` | – | crate `toolbox-kit` |

GitHub addresses do not care about upper and lower case; still, every file writes them exactly as in this table,
so a search finds all of them.

The Toolbox does not care about that folder: every program is added on its own with **Add program…** (4.2), from
wherever it is.

**What ties a program to this repo** is one line in its `Cargo.toml`:
`toolbox-kit = { git = "https://github.com/Windows-Toolbox/Toolbox" }`. Cargo fetches the library when building
and remembers the exact commit in the program's `Cargo.lock`; `cargo update` moves a program to the newest kit when
*you* decide.

**The common folder earns its keep through one file**, `Windows Toolbox\.cargo\config.toml`. Cargo reads such a
file from every folder above the project it builds, so it applies to all program repos at once and belongs to
none of them:

- **One shared build folder.** Without it every repo gets its own `target\` – several GB each, and egui compiled
  again for every program. With it, what one program has compiled the next one reuses.
- **The local kit.** On your PC it redirects `toolbox-kit` from the git address to the `kit\` folder on disk, so a
  change in the kit shows up in every program at the next build, without committing or `cargo update`. Someone who
  clones a single program does not have that file and simply gets the kit from git.

Both are standard Cargo settings and combine from a parent folder as described (M0: two repos below one
`.cargo\config.toml`, one shared `target\`, the local kit used by both; a single repo copied elsewhere fetched the
kit from git). Three things M0 showed:

- **The git address must already hold the kit.** Cargo fetches the git source even when `[patch]` redirects it,
  and fails if there is no `toolbox-kit` package there. So the kit is pushed to this repo (M1) before the first
  program depends on it.
- **On your PC `Cargo.lock` pins no kit commit.** With the redirect active, the lock file records the kit without a
  source; a program built here (also its release) uses whatever the local `kit\` holds. Only a single clone without
  the parent file pins a commit. That is wanted – the release is built from the kit you see – but it means a
  release build is only reproducible together with the kit's state at that time.
- The kit is found inside this repo's workspace (root `Cargo.toml` with `kit` as a member) by its package name.

The patch line in `Windows Toolbox\.cargo\config.toml`:

```toml
[build]
target-dir = "target"

[patch."https://github.com/Windows-Toolbox/Toolbox"]
toolbox-kit = { path = "Toolbox/kit" }
```

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
  "repository": "Windows-Toolbox/Hardware-Monitor",
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
| `update_key` | the public key its releases are signed with. An update is only installed if its signature fits the key of the version already installed (section 9). |
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
  and offers every program it finds there – on a new PC one click on `Windows Toolbox\` instead of one per
  program.
- **One `id`, one entry.** Two copies of the same program share pipe name and autostart entry, so only one can be
  on the list. Adding the second one asks which of the two to keep.
- **Scripts belong to the agent.** The Toolbox window is a short-lived process, so the agent (section 8) starts
  scripts and puts each one into a named Windows job object (`WindowsToolbox.<id>`). "Running" is then "the job
  has processes" – still true after the window or the agent was restarted – and Stop ends the script together
  with everything it started.
  M0 found the catch: a job's **name exists only while somebody holds a handle to it** – processes inside the job
  do not count. When the agent ended, the script ran on but could no longer be found. Fix (tested): the script is
  started suspended, put into the job, and **inherits one handle to its own job** (and only that one, through a
  handle list); then it runs. The job stays findable as long as the script's first process runs. If that process
  ends while something it started still runs, the leftovers are no longer found – acceptable for scripts that
  stay in the foreground of their own work, as Rayman's does.

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
the logged-in user and nobody else – stricter than today's open TCP port. (M0: security descriptor
`D:P(A;;GA;;;<user SID>)` plus `PIPE_REJECT_REMOTE_CLIENTS`. Pipe handles are opened overlapped: on a synchronous
handle Windows serialises all I/O, and a reader waiting for the next line would block every write.)

The exact messages are listed in section 15.1.

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
| `autostart` | `winutil.py` (PowerShell) | Run key – direct API calls, name derived from `id`; also reads whether Windows has the entry disabled (Task Manager → Startup, section 8) |
| `process` | `winutil.py`, starters | start hidden, run a helper elevated through its scheduled task (no prompt), wait for exit, job objects for scripts, efficiency mode (EcoQoS) |
| `update` | `git pull` + Setup | one function for every kind of program and the Toolbox itself: look for a release, download, check its signature, swap files; the same code installs a program from "Get programs" (section 9) |
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
  `stop`. With the workers off it ends as soon as **nobody who wants its `page`** is connected any more (after
  2 s). Not "nobody at all": the agent is connected to every running program for status and tray, and would keep
  a stopped program alive forever (found in M0). That way exactly one process holds the pipe and writes
  `settings.json`, and Start can never mistake an open settings page for a running program.
- **window role**: an egui window (the detail window; the settings page when there is no Toolbox). A second process
  of the same `.exe`, started by the background role on a click or on `show`; it ends when the window closes and
  gives all its memory back.

Double-clicking a program always means: *make sure it runs, then show its window.* Programs without a background
part (Cube Game) simply have no background role.

Why two processes instead of one hidden window: a hidden egui window keeps its renderer in memory, and egui's event
loop is known to be awkward to wake from tray events while its window is hidden. Two roles avoid both by
construction. Confirmed in M0 (release build, glow):

| Process | Private memory | CPU while idle (10 s) |
|---|---|---|
| Toolbox agent (tray, pipe) | ~4 MB | 0 ms |
| Toolbox window (open) | 70–125 MB, mostly the graphics driver | 0–16 ms |
| a kit program, workers off, serving its page | ~2.5 MB | 0 ms |

The window's memory is given back completely when it closes – the reason for the split. A single process that
creates and destroys its window was therefore not pursued.

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

**Without the Toolbox** a background program must still be reachable (how else to quit it?). Rule in `kit::app`:
while its workers run and no Toolbox agent carries its tray menu for 1.5 s (a Toolbox that is there connects within
that moment of the start), the program shows the same data in an icon of its own – the status line as tooltip,
its menu entries, then **Settings** (its page in the Toolbox; if none runs, a message naming the settings file)
and **Quit**. The moment the agent connects, that icon disappears. No polling: the agent connects to every running
program when it starts, a program connects to the agent when it starts, and a closed pipe tells either side that
the other one is gone.

**Windows 11 hides new tray icons** in the overflow (`^`) until the user drags them out or switches them on under
Settings → Personalisation → Taskbar → Other system tray icons. That goes for the Toolbox icon and for every live
display. Two consequences:

- The steps are described where they are needed: on the Toolbox's settings page and right next to the switch
  "Temperatures in the taskbar", with a button that opens that Windows settings page.
- Windows remembers the choice per icon, under `HKCU\Control Panel\NotifyIconSettings`, keyed by **exe path +
  icon uid**. Each icon therefore gets a fixed uid that never changes: 1 = Toolbox icon, 2, 3, … = live displays
  (no GUIDs – Windows ties a GUID to one exe path and refuses it after a move). M0 measured:
  - **Update** (exe renamed aside, a different build at the same path): same entry, the choice is kept.
  - **Moved folder**: a new entry, the choice is lost. The Toolbox's settings page and the live-display switch say
    so ("after moving the folder, switch the icons on again").

Cost while idle: a running program pushes `tray` only when its line changes (the Hardware Monitor at its icon
rate, Discord Plus a few times a day); the agent wakes for that message and sleeps again.

---

## 7. Look and feel

egui through **eframe** (the standard way to run egui as a normal Windows program; Last Bastion's editor uses egui
through macroquad instead, which redraws every frame – right for a game, wrong for a tool).

- **Reactive drawing**: egui only redraws on input or when asked (`request_repaint_after`). A page with live values
  asks for a redraw at its refresh rate while visible; everything else draws nothing while idle.
- **Renderer**: `glow` (OpenGL), decided after M0 (release build, same window):

  | | glow | wgpu |
  |---|---|---|
  | Toolbox.exe | 7.6 MB | 12.9 MB |
  | window drawn, from process start | 230–380 ms | 770–875 ms |
  | private memory of the open window | 70–125 MB | ~395 MB |

  On a PC where OpenGL is missing or too old (virtual machine without graphics driver, very old driver), creating
  the window fails. The kit catches that and shows a plain Windows message box saying what is missing ("this PC
  has no usable OpenGL – install the graphics driver"), and writes it to `log.txt`. No wgpu fallback.
- **eframe features**: `default-features = false`, then `glow`, `default_fonts`, `system_fonts`, `clipboard`,
  `links`, `accesskit`.
- **Theme** in `kit::ui::theme(ctx)`, applied by every window – the editor's `style()` carried over: egui's dark
  visuals, text sizes 12 / 15 / 15 / 19 / 14 (small / body / button / heading / monospace). What is chosen is
  egui's own blue, as in the editor; headings are blue too (`ACCENT`, a lighter tone of it). Colours for good /
  bad / warn come along as constants. Light/dark/system and the zoom are settings of the Toolbox; programs
  receive them in `hello` (4.3) and use them for their own windows.
- **The editor's building blocks** are in the kit, for the Toolbox and for every program's own window:
  `heading` (a page's title), `section` (a blue heading that folds shut with a click, its content indented –
  what a page's `section` block is drawn with), `hint` (small grey text), `bar_button` (the module bar's buttons).
- **Layout** of the Toolbox window is what the editor already does: a bar at the top like its module bar
  (category tabs, on the right "Settings"), `SidePanel` (programs of the tab), `CentralPanel` (the page), and a
  status line at the bottom (left: link target on hover, toasts; right: how many programs run, size, version).
- Solved along the way: the Tk scroll stripes (egui scrolls by redrawing, there is nothing to smear), zoom
  (`Ctrl` + wheel is built in), DPI scaling, tooltips. The planned PySide6 port is no longer needed.
- **Symbols**: egui 0.37's own fonts lack `●`, `✓` and `▸` (its emoji fonts are an extra feature and still lack
  them). eframe's `system_fonts` takes missing characters from the Windows fonts (Segoe UI Symbol) – everything
  used today is there, nothing to ship. A kit test checks the list (section 13).

---

## 8. The Toolbox itself

`Toolbox.exe`, two roles as in section 5.

**Agent (background)** – started by the first double-click, or with Windows if that is switched on (Run key):
the one tray icon with every program's menu and status (section 6), pipe `windows-toolbox.toolbox`
(`show [id]`, `quit`, starting and stopping scripts – 4.2). It sleeps until a click or a message from a program
arrives. Kit programs push their status. What cannot report by itself – a script (its job), a plain exe (its
process), a Python program (its port) – is looked at when it is needed: when the tray menu opens, and once a
second while a window is open. With the window closed there is no timer at all.

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

### What Windows shows

- **Task Manager, processes**: every exe carries its name, icon and description as a compiled-in resource
  (section 9), so the list reads "Hardware Monitor", not "pythonw.exe".
- **Task Manager, Startup tab**: an autostart entry appears there under the same name – and can be disabled
  there, which leaves the Run key in place. The kit reads that state too. "Start with Windows" then shows
  "switched off by Windows" with a button that switches it back on, instead of claiming "on".
- **Taskbar**: agent and window are two processes of one exe. A fixed application id (AppUserModelID,
  `WindowsToolbox.<id>`, set first thing in `main`) makes Windows treat them as one program, so pinning the window
  to the taskbar and clicking the pin work as expected (M0 hand test: works). The pin shows the exe's name and
  icon – in the prototype "toolbox" and the generic icon, because it had no resource yet; with the resource of
  section 9 it reads "Windows Toolbox" with its own icon.
- **Start menu**: a switch "Show in the Start menu" in the gear (off by default) creates one shortcut to
  `Toolbox.exe`; it is repaired after a move like every other path (section 2).
- **Settings → Installed apps**: deliberately no entry. The Toolbox is portable; it is removed by Gear →
  Uninstall and deleting the folder (section 9).

### Start-up cost

Today: Python interpreter + customtkinter + importing every program's page. New: one native exe that
reads a handful of small JSON files. Measured in M0, from process start to the drawn window: today's Toolbox
~1080 ms, the new one 230–380 ms – including starting the program behind the shown page and receiving its page.
The agent is ready ~40–50 ms after its start.

---

## 9. Install, update, uninstall

### Install

Today: clone → `Setup.exe` → Python via winget → copy 108 MB → pip → compile starters → downloads → admin steps.

New: **download the release zip, unpack, double-click `Toolbox.exe`.** There is no setup program. Nothing to copy,
nothing to compile, no Python, no Rust – Rust is only needed by someone who builds. Nothing else has to be on
the PC either: the Visual C++ runtime is linked into every exe (see "Building"), and the C# bridge uses the .NET
Framework that is part of Windows.

**Programs come with one click: "Get programs".** The Toolbox window has a list of the known programs (a small
signed file, `programs.json`, in this repo; loaded from GitHub when the list is opened – never on its own). A
click on one downloads its release zip, checks it, unpacks it into `programs\<id>\` next to `Toolbox.exe` and
puts it on the list – the same code as an update. So a new PC needs one download by hand, the Toolbox; everything
else happens inside it. Unpacking a zip yourself and **Add program…** stay possible for programs that are not on
that list.

**The two classic mistakes are caught.** If `Toolbox.exe` (or any kit program) is started from inside the zip
preview – Windows then runs it from a temporary folder – or from a folder it cannot write to, it would lose its
settings. The kit notices both at start and says what to do ("unpack the zip first", "move the folder out of
Program Files") instead of running half-working.

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
`program.json` into its `dist\` (the running-exe rename trick from Last Bastion's `build.ps1`; it never deletes
`dist\`, section 3). Requirement: Rust, already installed.

Two settings are the same in every repo and come with `template\`:

- **The Visual C++ runtime is linked in** (`crt-static`, in the repo's own `.cargo\config.toml`). Otherwise an
  exe needs `VCRUNTIME140.dll`, which a fresh Windows may not have – and then it does not start at all. M0 checked
  the import tables: with it, the exes import only Windows' own DLLs.

  ```toml
  [target.x86_64-pc-windows-msvc]
  rustflags = ["-C", "target-feature=+crt-static"]
  ```
- **A release profile** in `Cargo.toml`: `lto = true`, `strip = true`, `codegen-units = 1`. Result in M0: the
  Toolbox with egui 7.6 MB, a program without window (background role only) ~0.6 MB.

**A release** is made by the second file, `release.ps1`: it runs `build.ps1`, zips `dist\` without `data\`,
**signs the zip**, and publishes zip and signature as GitHub release `v<version>` (`gh release create`), the
version taken from `Cargo.toml`. It refuses if that version was already released or the repo has uncommitted
changes.
Name, icon and description in Task Manager come from a resource compiled into each exe by a build script – that is
all `make_exe.py` and `starter.cs` were for.

### Update

Today `git pull` + `Setup.exe -Restart`. With compiled programs a user PC has no source, so:

- **User PC**: on a click on "Check for updates" (automatic checking is a switch, off by default) the kit asks
  GitHub for the latest release of *that* repo (the manifest names it:
  `"repository": "Windows-Toolbox/Hardware-Monitor"`), downloads the zip, checks its signature, swaps the files (a running
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

**Updates are signed.** A checksum lying in the same release as the zip would only catch a broken download:
whoever can replace the zip can replace the checksum. So `release.ps1` signs the zip with a private key
(Ed25519) that lies on the developer PC outside every repo, and the public key stands in the program's manifest
(`update_key`; kit programs get it from the kit). An update is installed only if its signature fits the key of
the version **already installed** – a taken-over GitHub account alone cannot push an update. `programs.json` is
signed the same way and carries each program's key, so the first install from "Get programs" is checked too.
The price: the private key has to be kept safe and backed up. If it is lost, the next version has to be
downloaded by hand once. The checksum of LibreHardwareMonitor is pinned in the program's code and needs none of
this.

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
| **Cube Game** | Python, pygame-ce | largest | ported 2026-11-18 (M5, ahead of M4): own Win32 windows + tiny-skia, not macroquad – see M5 in section 11 |

### Hardware Monitor – the one hard question

**LibreHardwareMonitor stays exactly what it is**: the developers' unmodified `LibreHardwareMonitorLib.dll`,
downloaded in a pinned version with checksum, and updated by changing version + URL + SHA-256 in one place – as
today. PawnIO likewise. Only our own code (today ~240 KB of Python around it) is rewritten.

The question is only how Rust *calls* a .NET library – today pythonnet does that:

| | How | For | Against |
|---|---|---|---|
| **A. Bridge process** (chosen) | a tiny C# program `LhmBridge.exe` (one file, ~150 lines) loads the DLL and passes values on as JSON lines over a pipe: `open`, `read`, `set_fan` | LHM untouched and isolated; a new LHM version needs no change on our side as long as its sensor names stay | the one piece of own code that is not Rust; one more process with the .NET runtime in memory (pythonnet loads the same runtime today); needs a C# build – the `csc.exe` shipped with Windows (`Framework64\v4.0.30319`, C# 5) is enough (M0: 7 KB exe against LHM 0.9.6, no extra config file) |
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
  of your user can reach through it.
  M0 tested the whole path with one admin prompt: bridge + LHM copied to `C:\Program Files\WindowsToolbox\…`
  (writable only by TrustedInstaller, SYSTEM, Administrators – a normal process could not replace the exe), a task
  without trigger and with highest privileges, started by a normal process without a prompt, the bridge elevated
  and connected after 0.7–1 s, CPU temperature read (without admin rights it reads 0). A pipe with the plain
  user-only access rule works from the normal process to the elevated one – no integrity label is needed. The
  bridge ended as soon as the connection closed.
- **Fans go back to automatic** when the connection closes, for whatever reason: before it ends, the bridge hands
  every fan it has set back to the mainboard. A crashed monitor never leaves a fan stuck at its last speed.
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

**M0 – Prototype (throw-away) – done 2026-11-16.** The code lies outside this repo (`E:\Software\toolbox-m0\`, a
kit, a Toolbox, a demo program, the bridge, the measuring scripts) and is not carried over; M1 starts fresh from
section 15. Every point below is answered in the section it belongs to. Hand test 2026-11-16: the window on the
real screen is scaled correctly, slider, tray menu, page and pinning to the taskbar work – the off-screen screenshot's 125 % content in a 100 % window was only an effect of the window lying
where no monitor is.
The list as it was planned:

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
- one application id for agent and window: pinning to the taskbar, grouping
- the window on a PC without usable OpenGL (a virtual machine is enough)
- `crt-static`: the built exe no longer asks for `VCRUNTIME140.dll`; size with and without the release profile

Result, written into this file: measured numbers, corrections where an assumption was wrong, and **the formats**
(section 15) – so that M1 starts from facts and fixed formats instead of sketches. The mini page ran through the
whole chain as an automated test without a window: workers off → `set` pushed back at once → ten slider steps
written to the file once, after the drag → an invalid value refused with a toast → a second start hands `start`
to the running process → `quit`. How the slider feels by hand is part of the open hand test.

**M1 – Kit + new Toolbox, next to the old one – built 2026-11-16.** Checked on the real PC: the window and
the tray entry (by hand); stopping and starting a Python program, the classic settings, a script's option and
adding a program (the window's messages sent to the running agent – the buttons themselves were not each clicked). `kit\` (`app`, `settings`,
`ui`, `i18n`, `ipc`, `tray`, `manifest`, `page`, `process`, `proto`, the build helper) and `toolbox\src\` in this
repo, `Toolbox.exe` with library, page display, script page, tray menu, **Add program…**. The Python programs are
added from their current folders and appear as they are: started through their existing starters, status through
their existing ports (the kit's IPC client understands the old `"port"` field until the last Python program is
gone). Their "Settings" button opens the **old** Toolbox on that page – the `show <id>` command it already has.
The Python programs keep their own tray icons until they are ported; the single icon fills up program by program.

What M1 showed or settled:

- **The two Toolboxes do not meet.** The new one is built into `dist\Toolbox.exe`; the old one stays exactly
  where and what it is – `Toolbox.exe` in the main folder with its shortcuts, its autostart and the ⚙ of the
  Hardware Monitor pointing to it. So nothing had to be renamed to `ToolboxClassic.exe`; the new Toolbox looks
  for that name first and for `Toolbox.exe` second, should a later step rename it.
- **First start next to the old Toolbox**: language, look and the installed programs are carried over once
  (`data\toolbox.json`, `data\installed.json`), so the new window opens with everything in it.
- **A script still lying in the old layout** (Rayman until M2) is started the way the old page did it – its
  options as arguments (`-Pos Left`) – and its option file is the old `data\<id>.json`. A script in the new layout
  gets no arguments and reads its own `data\settings.json` (15.3).
- **Window and agent talk over the agent's pipe.** The agent holds the list of programs and everything known
  about them and sends it to the window on every change; the window sends back what the user did. These messages
  are private to `Toolbox.exe` (15.1).
- **Still to come, each with the milestone that needs it**: "Start with Windows" (built in M3, see there) and
  "Restart after a crash" (M3, with `kit::autostart`), Update and **Get programs** (M3), Set up / Uninstall (M4), an "Open" entry for a
  program's own window in the tray menu (M4), export / import, Start menu shortcut, clean up (M5). The gear holds
  look and language, the list of programs, the tray icon's instructions and Quit – and since 2026-11-17 **Restart
  the Toolbox** (also in the tray under Quit ▸): the new agent gets the old one's process id (`--after <pid>`) and
  waits until it has ended, otherwise it would find the old pipe and end at once; it starts the exe by its name, so
  after `build.ps1` the new build runs; an open window opens again; the programs keep running.
- **A window's icon**: eframe puts egui's own icon over ours one frame after the window appears. `kit::ui::viewport`
  hands eframe an empty icon, which makes it leave the window alone; `use_program_icon` then sets the one
  compiled into the exe.
- **Tests**: 47, all without a window – among them the agent in a sandbox, talked to the way the window does
  (a script from add to remove; a kit program followed, crashed, followed again), and every page of the window
  drawn into memory (section 13).
- **Size**: `Toolbox.exe` 8.6 MB, Kit Demo (no window) 0.7 MB – release build.

**Built ahead for M3 (2026-11-16, after the M1 commit) – in the kit, not yet used by the Toolbox.** The parts of
M3 that do not need Discord Plus itself: `kit::autostart`, `kit::http`, `kit::update` and the `sign` tool with
the release key (15.6, 15.7); a kit program's own tray icon when no Toolbox runs (section 6); the `entry` block
(15.2, Discord Plus' login); `Runtime::sleep`, so a changed setting or Stop does not wait for a worker's pause;
actions that change settings (a switch in the tray menu); `copy_text`. Each with tests, all without a window or
the network – 61 in all. What is still missing for M3: the Toolbox's switches and buttons that use them ("Start
with Windows", Update, **Get programs**), `programs.json`, `release.ps1`, and Discord Plus.

**M2 – Rayman + Last Bastion – built 2026-11-17.** Rayman moves into its own folder (two files) and proves the
script page; Last Bastion gets its `program.json`. First two programs outside this repo that no longer need the
classic Toolbox – and that the classic Toolbox no longer shows: `programs\rayman_legends\` and
`programs\last_bastion\` are deleted here.

What M2 showed or settled:

- **Rayman Legends** (`..\Rayman Legends\`, repo `Windows-Toolbox/Rayman-Legends`): `program.json` with all texts in
  it (15.3 – no `i18n.json`), `rayman_borderless.ps1` without parameters – it reads `Pos` from its own
  `data\settings.json` at the start and while the game runs. Next to them only `.gitignore` (`/data/`) and
  `KONZEPT.md`, the older concept for a successor ("Screen Zones", written before this document; not planned yet).
- **A script's options move with it.** When a script in the new layout is added (or replaces its classic entry) and
  has no `data\settings.json` yet, the Toolbox writes the values the classic Toolbox saved for it
  (`data\<id>.json` there) into that file – once, only the script's own options, only valid values. So the position
  chosen before is still chosen.
- **Last Bastion**: `program.json` in its repo root (`exe`, texts, `button` "Play"), without `version`; its
  `build.ps1` writes it to `dist\program.json` with the version from `Cargo.toml` added, so the version stands in one
  place (a kit program gets this from `kit::build`; an external exe does it in its build script). The old page's
  extras – choosing the game folder, opening DESIGN.md – are dropped: the folder is the one added, and Open folder
  and README are on every page.
- **Switching over** is **Add program…** on the new folder; the Toolbox sees the same `id` already on the list and
  asks which one to keep. Tested on the built `Toolbox.exe` in a sandbox, through the window's messages.

**M3 – Discord Plus in Rust.** First kit program and first one built in a repo of its own against the kit's git
address: background role, its page shown in the new Toolbox (running and stopped), instant `set`, autostart, an
action button (Connect), its menu under the Toolbox icon and the fallback icon without it, its own signed release
zip and update, and **Get programs** in the Toolbox with Discord Plus as its first entry. `template\` in this
repo is extracted from it.

**M3, first part – built 2026-11-17.** Discord Plus lives in its own repo (`..\Discord Plus\`,
`Windows-Toolbox/Discord-Plus`); its README holds the port's checklist (section 14), written from the Python version
and checked line by line. In this repo:

- **"Start with Windows"** in the frame of every background kit program (a switch under Start · Stop · Restart).
  The agent reads `kit::autostart` for those programs with the list and again once a second while a window is open
  – Task Manager can switch an entry off at any time. *On* means: Windows starts **this** copy. An entry Windows has
  switched off shows "switched off in Windows – switching on allows it again"; an entry that starts another copy
  of the program shows which, and switching on takes it over. Switching off removes only an entry of this copy.
- **Remove from the list** also removes the program's autostart entry (only one that starts this copy) – the
  Toolbox would no longer show the switch to undo it.
- **A running classic program gives way.** When the answer to "which one to keep" replaces a running program of
  the classic Toolbox (Python, with a port), the agent asks the old one to quit on its port and then starts the new
  one – otherwise both would run (Discord Plus: the activity twice in Discord).
- **Tests**: 65 in this repo – new: the agent with the switch, another copy's entry and Remove (in the sandbox the
  entries lie where Windows does not look); a classic program played on a port giving way; the frame drawn with
  both hints.

**Hand test passed 2026-11-18** with the real Discord app and the real Toolbox (call status for a server voice chat
and a private call, Start with Windows taking over the Python version's entry, Stop); `programs\discord_plus\`
left this repo, as Rayman did in M2. Found on the way: a kit program started for its page and then switched on
with Start never runs `main` with its workers on – a one-time step at "the first real start" belongs where the
worker first runs (Discord Plus: taking over the autostart entry). Its checklist keeps ◐ for what the test did not
reach (Connect and renewing the token – the Python version's sign-in was still valid).

Still open for M3: Update and **Get programs** (`programs.json`), the first signed release of Discord Plus,
`template\`.

**Screen Zones – built 2026-11-17, between the milestones.** The successor of the Rayman script that `KONZEPT.md` in
the Rayman repo describes, as a kit program in its own repo (`..\Screen Zones\`, `Windows-Toolbox/Screen-Zones`):
any game or program gets an entry and lands borderless in its zone at every start; the concept's steps 1–3
(parity with the script, many games with "Add a game" from the open windows, keys for the active window – opt-in).
Step 4 (admin games, keeping the mouse in the zone, own zones) and the extensions are listed in its README.
What it settled here:

- **Keys may be paths** (`games.0.zone`, 15.2) – the first page with a list whose entries each have their own
  switches. `kit::page` only; the Toolbox passes keys on unchanged, so an older Toolbox shows the page as well
  (checked with the `dist\Toolbox.exe` of M3's first part, in a sandbox).
- **A worker with a Windows message loop**: window events and keys arrive as messages to the thread that asked for
  them, so the worker cannot sleep in `Runtime::sleep`. A small helper thread does and posts a message to the worker
  whenever it wakes (a `set`, an action, Start, Stop) – no polling, nothing new in the kit.
- **The concept's open decisions** (asked 2026-11-17): name "Screen Zones"; the page lives in the Toolbox (the kit's
  answer to the concept's "Python page or own window"); the exe is built, not kept in git (section 9); no tray
  icon of its own (section 6). The keys were first Win+Alt+arrows – on the developer's PC all four turned out to be
  taken already (most likely by Windows 11's own snapping), so the page offers a **choice**: Win+Ctrl+Alt (default,
  hardly ever taken), Ctrl+Alt (turns the screen with some Intel drivers), Win+Alt. Windows does not tell who holds
  a key; the program only finds out by trying, and gives every combination with a taken key a thin red frame – for
  that a `choice` option may carry a `state` (15.2).
- **Switching over**: at its first start it takes Rayman over from `..\Rayman Legends\` (entry, zone, Play target);
  the Rayman script is then removed from the list, and its repo is retired once the hand test with the real game
  passed.

**Screen Zones – many entries (built 2026-11-18).** With dozens of entries the page breaks down:
one section per entry, all open, in the order they were added, nothing to search. Worked out with a clickable
board (three designs, real and example entries); the user's decision: **all three, as views of one list**. First
built as three buttons List · Detail · Group; then (the user, the same day) "Group is just another list – why not
with Detail too?": **two switches** that combine, **Groups** and **Detail**, so four ways from the same entries.

- **A new kit block `list`** (step 1 of 4.5). The program sends the entries; the Toolbox draws them:
  - one row per entry – state dot, title, subtitle, tags, one quick button, an arrow;
  - *Groups* off: one flat list, chips "All" + one per group above it; on: one folding heading per group with its
    count (groups the program marks `open` start open, groups without an entry are left out, no chips);
  - *Detail* off: a click opens the entry's blocks under its row, one at a time; on: compact rows on the left
    (the state's text under the title), the chosen entry's blocks on the right – under each other below 560 px.
- **Searching, sorting and the switches are the Toolbox's own business** – nothing goes to the program, so typing
  costs no round trip, and the program never needs to know how its list is shown. A filter field (Ctrl+F, Esc
  clears it) matches title, subtitle and tags; a sort menu offers "Active first" (not with Groups – there it is
  A–Z), the program's own sorts and A–Z. Switches, filter, sort, open row and chosen entry are kept in egui's
  memory under the list's `key` (by the entry's `id`) – a page pushed again (a game starts) and leaving the page
  keep them, for as long as the Toolbox runs; it starts again with both switches off.
- **Fields**: 15.2. Kit: `page::Builder::list` / `section_closed`, the renderer in `kit\src\ui\list.rs` (filter
  and sort are plain functions with tests).
- **Rows do not jump under the mouse**: when a pushed page would reorder the rows while the pointer is over the
  list, the new order waits until it leaves. A new or just-started entry flashes once.
- **`section` gets `open`** (default true, as today): Keys, Good to know and Advanced start folded.
- **Screen Zones' side**: the list replaces the sections per game and "Add a game" (now the list's `add`); groups
  Active / Needs attention / Others / Off; a sort "Last played" from a new field `last` per entry (set when its
  window is taken). An older Toolbox shows "needs a newer Toolbox" for the list – Toolbox and Screen Zones are
  updated together.
- **Tests**: filter, sort and the steady order as plain functions; all four ways drawn without a window, wide and
  narrow, with and without entries; the keys of the list's blocks (`games.3.zone`) through `check_keys_with`.
  Photographed in a sandbox Toolbox: plain, Detail (wide and narrow), Groups, Groups with Detail.
- **Found on the way**: a slider or text field next to a long label made the page wider than a narrow column (the
  view Detail) – the overflow widened every block after it. Both now put the label above the control when the two
  do not fit side by side.

Found on the way: **Citrix's viewer takes "always on top" away by itself**, so a held window must not trust what it
set a second ago – Screen Zones now reads the real flag in its regular look, and a window owned by the held one
(Citrix lays an overlay of a second process over its session) counts as the held one being active. This applies to
every entry, not only to virtual machines: it costs one call per held window and second and changes nothing for a
window that keeps the flag.

**Screen Zones – more per game (concept 2026-11-18, not built yet).** Worked out on the same board as Steam
Shortcuts (below); what stays with Screen Zones because it is about windows:

- **Recognised by: several programs** (`exes`, a list instead of `exe`; an old `exe` is read as a list of one).
  Each is put into the zone; one may be marked *launcher* – started, never placed. Added from the open windows, as
  "Add a game" does.
- **Launch parameters** for "Play" (`-windowed`).
- **Mouse kept in the zone** while the game is active (`ClipCursor`; the README's "keeping the mouse locked in") –
  a switch per entry, from step 4 of the concept.

**Steam Shortcuts – built 2026-11-18.** The user's problem: the Steam Controller works only in games started through
Steam. First planned inside Screen Zones (the board, both decided that way), then – with "point the desktop icons at
Steam, and back" added – **a program of its own** (the user, the same day): its list comes from desktop shortcuts and
programs, most of which need no zone, and nothing has to pass between the two – Screen Zones recognises a game by
its program name, so a game started through Steam still lands in its zone; Screen Zones' "Play opens" can simply
hold the Steam address (Steam Shortcuts has "Copy the Steam address"). Own repo `..\Steam Shortcuts\`, a kit
program with a background worker; its README holds the details and the checklist. What it settled:

- **Steam's list** of non-Steam games is `userdata\<account>\config\shortcuts.vdf` (binary VDF); the account is
  the one signed in now (`HKCU\Software\Valve\Steam\ActiveProcess\ActiveUser`), else the last one
  (`config\loginusers.vdf`). The program reads and writes the whole file, keeps every part it does not know byte
  for byte, and only adds, changes and removes entries with its own numbers – a test rebuilds the user's real file
  from what was read, byte for byte, without writing it. A copy goes into `data\steam-backup\` before every write.
- **Only while Steam is closed** – Steam writes its own copy back when it quits. Steam closed: written at once.
  Steam running: per entry **"Restart Steam and add"** (`steam.exe -shutdown`, wait, write, start) or **"Add when
  Steam is closed"** (the user's choice instead of a global "collect" mode): the entry waits, a line above the list
  names everything waiting with "Restart Steam now", and the worker (it looks every two seconds) writes it the moment
  `steam.exe` is gone. Steam is never quit without a click. What waits is not stored separately: an entry *wants*
  to be in Steam (or not), the list is read, the difference is what waits – an entry changed while in Steam waits to
  be written again.
- **Numbers**: made once as Steam made them for years (CRC-32 of exe and name, top bit set), kept afterwards;
  `steam://rungameid/<number << 32 | 0x02000000>` starts the entry.
- **Addresses** (`uplay://`, `battlenet://`, `com.epicgames.launcher://`) go to Steam as the program Windows opens
  them with (`HKEY_CLASSES_ROOT\<scheme>\shell\open\command`), the address as option: what a launcher started
  through Steam starts gets Steam Input too – tried by the user with Ubisoft Connect and Rayman Legends. Not yet
  checked: a launcher already running outside Steam probably takes the game over without Steam Input; the page says
  so.
- **Desktop**: `.lnk` files are read directly (MS-SHLLINK: link info, strings, environment block), `.url` as text.
  Pointing an icon at Steam moves the original into `data\desktop-backup\<entry>\` and writes a `.url` with the
  same name and icon; putting it back restores the very file. A `.url` is only ever deleted if it holds a
  `steam://rungameid/` address. Shortcuts on the desktop of all users need administrator rights – the page says so
  instead of asking for them (no bridge for this).
- **Icon places** (the user's wish after the hand test): a replaced shortcut is a new file to Explorer and would land
  on the next free place, so the old one's place is read before and set after – through Explorer's `IFolderView`
  (`GetItemPosition`, `SelectAndPositionItems`), the only part that needs the `windows` crate (COM wrappers
  windows-sys lacks). Checked on the real desktop with a throw-away shortcut moved away from the first free place.
- **Hand test passed** (2026-11-18): Rayman through Ubisoft Connect into the real Steam, Steam restarted, desktop
  icon pointed at Steam and back, the controller worked.

**M4 – Hardware Monitor in Rust.** Bridge as the only elevated part, status line and live displays in the tray,
detail window (with the new blocks and the canvas from 4.5), settings, setup steps (PawnIO, LHM, task).
After this, `ToolboxClassic.exe` has no page left to show and is deleted together with `toolbox\*.py`.

**M5 – Cube Game in Rust; Python leaves.** Then `runtime\`, `requirements.txt`, `setup\`, `Setup.exe`,
`Uninstall.exe` are deleted and the release zip + updater (section 9) become the only way to install.

**M5, the game – built 2026-11-18, ahead of M4** (the user's decision: getting rid of `runtime\` was one of the main
points). Cube Game lives in its own repo (`..\Cube Game\`, `Windows-Toolbox/Cube-Game`), ported 1:1 from the Python
version – same behaviour, same data files (`content\`, read from the repo on the developer PC, compiled in
elsewhere), same texts; its README holds the checklist. What it settled:

- **Not macroquad.** The game window *is* the arena: position, size and picture change every frame, in one step
  (`UpdateLayeredWindow`) – done separately, Windows redraws in between and the window jumps (the Python version
  measured flicker in 56 % of moving frames). It also opens passive extra windows (watchtower, radar) that never take
  the focus, and locks the mouse into several rectangles. macroquad has one GPU window and none of that. So: own
  Win32 windows (`windows-sys`, as in the kit), drawing in software with **tiny-skia** straight into the window's
  bitmap (channel order blue/green/red throughout, so nothing is copied), text with **fontdue** in pygame's built-in
  font. pygame's rules are kept where the game's look rests on them: `Rect` cuts values off, `FRect` is `f32`, shapes
  replace pixels and only blits mix. Checked: menus and skill tree pixel-close to the Python version, the first two
  seconds of a run equal to the decimal.
- **A kit program without background role.** Play (or a double-click) switches the workers on: the worker runs the
  game on its thread until the window closes; Stop closes the window. For that the kit got `Runtime::stop` – the
  worker ends its work itself (15.7). With nobody looking at the page the program then ends with `bye`.
- **Progress** in `dist\data\profile.json`, carried over once from the classic Toolbox's `data\cube_game.json`; the
  page's two settings in the kit's `settings.json`; own enemy pictures in `dist\data\images\`.
- 3 MB instead of a Python runtime of 115 MB. Tests without a window: a bot plays every level for 2½ minutes with the
  whole skill tree and draws every frame into memory.

Open: the hand test (Play, Stop, playing a run in the real window); then `programs\cube_game\` leaves this repo, and
after M4 `runtime\` and `setup\` with it.

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
| Install on a new PC | clone, Setup.exe, minutes | unzip, double-click; programs with one click each from "Get programs" |
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
write to; "Restart after a crash" exists as a switch, off by default; "Get programs" is part of the core (M3),
not a later extra; releases are signed with an own key.

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
- **Symbols**: every symbol the pages use (`●○⚠⚒⚙✓▸°·…`) is found in egui's fonts or the Windows fonts – an egui
  context without a window, with the system font provider, asked per character.
- **A sandbox for everything that starts a process**: the internal environment variable `TOOLBOX_SANDBOX` gives
  every name Windows knows (pipes, jobs) a suffix, shows no tray icon and no message box, and puts a window off
  the screen; `TOOLBOX_DATA_DIR_<ID>` gives one program another data folder. So a test never meets the Toolbox or
  the programs that are really in use. A test that waits for an answer has a time limit – a wrong expectation
  must fail, not hang.
- **Off-screen when a window is needed** (measurements, a screenshot): in the sandbox, or with the internal
  environment variable `TOOLBOX_OFFSCREEN`, the window lies at −20000/−20000, without taskbar entry and never
  activated; `PrintWindow` captures it. Scaling is not trustworthy there (no monitor) – a drawing check at real
  scale stays a hand test.
- **Setup steps**: each step's "already done?" check runs without changing anything.
- **The drawing itself**: egui draws into memory instead of a window. The Toolbox's whole frame – every kind of
  program, the gear, the empty library, dark and light – is drawn that way in one test; it catches a crash in
  drawing code and checks what the window chooses by itself (page, tab). Whether it *looks* right stays a hand
  test. (Mouse messages posted to an off-screen window do not arrive as clicks – M1 tried.)

All of it runs with `cargo test`, without admin rights and without a window.

---

## 14. Working from this document

This file fixes the direction. Before it was enough to build from without deciding things on the way, three gaps
had to be closed – in this order:

1. **M0 writes down the formats.** Done: section 15 – every building block with its fields; the exact JSON of
   `hello`, of every state and every command; the fields of `program.json`, of `[package.metadata.toolbox]` and of
   `programs.json`; how a release is signed; the public functions of each kit module.
   Two sessions must not be able to invent two different formats.
2. **M0 replaces every *(verify)*** by what was measured or found, and names the crates and versions that were
   actually used (looked up at that time, not taken from memory). Done;
   crates in 15.7.
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

---

## 15. Formats

Fixed after M0. Each list is complete for protocol 1; a later protocol only ever **adds** (a new message, a new
field, a new block type) and never gives an existing one a new meaning. Both sides ignore what they do not know.

### 15.1 Pipe messages

One JSON object per line (UTF-8, `\n`), each with a `type`. Pipe names: `\\.\pipe\windows-toolbox.<id>` for a
program, `\\.\pipe\windows-toolbox.toolbox` for the agent. Both sides use `min(own protocol, other protocol)`.

**To a program** (from the agent or the window):

| `type` | Fields | Example |
|---|---|---|
| `hello` | `protocol` (number), `language` (`"de"`, `"en"`), `theme` (`"dark"`, `"light"`, `"system"`), `zoom` (number, 1.0 = 100 %), `want` (list of `"status"`, `"page"`, `"tray"`) | `{"type":"hello","protocol":1,"language":"de","theme":"dark","zoom":1.0,"want":["status","page"]}` |
| `set` | `key` (string), `value` (any JSON) | `{"type":"set","key":"icon_interval","value":1.0}` |
| `action` | `name` (string) | `{"type":"action","name":"connect"}` |
| `start`, `stop`, `show`, `quit` | – | `{"type":"start"}` |

The first message on a connection must be `hello`; a program closes a connection that starts with anything else.
A later `hello` replaces language, look and `want` of that connection.

**From a program**:

| `type` | Fields | When |
|---|---|---|
| `hello` | `protocol`, `id`, `version` (string), `pid` (number) | first, as the answer to the caller's `hello` |
| `status` | `running` (bool: workers on), `state` (`"ok"`, `"warn"`, `"bad"`), `text` (string, may be empty) | after `hello` if wanted, then on every change |
| `page` | `blocks` (list, 15.2) | after `hello` if wanted, then on every change |
| `tray` | `line` (string), `menu` (list of `{"label","action"}`), `live` (list of `{"text","state"}`, at most 3, text at most 3 characters) | after `hello` if wanted, then on every change |
| `toast` | `text` | once, only to the connection whose command caused it (a refused `set`, the result of an `action`) |
| `bye` | – | to every connection, right before the program ends on purpose |

**Live displays** (`live` in `tray`): each entry becomes one extra tray icon of the Toolbox (uids 2, 3, 4 – section
6), shown only while the program's switch for it is on. The icon is the text drawn large in the colour of its
`state` (`ok` green, `warn` yellow, `bad` red) on a transparent ground – today's CPU/GPU icons of the Hardware
Monitor, e.g. `[{"text":"52","state":"ok"},{"text":"81","state":"warn"}]`. The limits come from the size of a tray
icon (16×16 pixels at 100 % Windows scaling, 32×32 at 200 %): two digits are well readable, a third one (`100`)
only just; more characters are cut off. At most three icons per program, so one program cannot fill the taskbar;
what the icons show and where the colour changes is decided by the program.

A refused `set` is answered with a `toast` and the unchanged `page`, so a control that already moved snaps back.
A slider's `set` carries a float; an integer field accepts it when it is whole.

**To the agent** (`windows-toolbox.toolbox`; no `hello` needed):

| `type` | Fields | Effect |
|---|---|---|
| `show` | `id` (optional) | open the window, on this program's page if given |
| `started` | `id` | a program announces itself when it starts its workers; the agent then connects to it like to every running program (no polling, section 6) |
| `quit` | `all` (bool, optional) | end the agent – with `all`, every background program first |

The Toolbox's own window uses the same pipe for more (`window`, `add`, `start`, … and the agent's answers
`library`, `toast`, …; `toolbox\src\talk.rs`). Those messages are **private to `Toolbox.exe`**: window and agent
are always the same build, so they may change with any version and no program may rely on them.

### 15.2 Building blocks of a page

Every block is an object with a `type`. A `key` is a field of the program's settings struct; `value` is its current
value, filled in by the kit. A key may also be a **path** into the settings, its parts joined by `.`: a field of a
field, or an entry of a list by its number – `games.0.zone` is the field `zone` of the first entry of the list
`games`. So a program with a list of things (Screen Zones: one entry per game) gives each entry its own switches.
The Toolbox only passes keys on, so this needs no newer Toolbox; a `set` for an entry that no longer exists is
refused like an unknown key.

| `type` | Fields | Drawn as |
|---|---|---|
| `section` | `title`, `blocks` (list) | blue heading that folds shut with a click, then its blocks, indented |
| `text` | `text` | a paragraph |
| `status` | `text`, `state` (`ok`/`warn`/`bad`) | a line with a coloured `●` |
| `switch` | `label`, `key`, `value` (bool) | a checkbox; sends `set` with true/false |
| `slider` | `label`, `key`, `value`, `min`, `max`, `step` (numbers), `unit` (string) | a slider with its value; the slider rule of 4.3 |
| `choice` | `label`, `key`, `value`, `options` (list of `{"value","label"}`, optionally `"state"`: `warn`/`bad`) | one button per option, the current one selected; an option with a `state` gets a thin frame in that colour (added 2026-11-17 – an older Toolbox shows a plain button) |
| `entry` | `label`, `key`, `value` (string), `secret` (bool, default false) | a one-line text field, dots if `secret`; sends `set` once when the field is left (click elsewhere, Tab, Enter) and only if the text changed – not per keystroke. While typing, pushed pages do not overwrite the field. |
| `buttons` | `buttons` (list of `{"label","action"}`) | a row of buttons; each sends `action` |
| `list` | `key` (string, tells lists apart), `items`, `groups` (list of `{"id","label","open"}`, in their order), `sorts` (list of `{"id","label"}`), `add` (`{"label","blocks"}`, optional), `empty` (text while there is no item). An item: `id` (stable), `title`, `subtitle`, `state` (`ok`/`warn`/`bad`/`unknown`) and `text` (the dot and what it means), `group`, `tags` (list of short texts), `button` (`{"label","action"}`, optional), `order` (`{"sort id": number}`, smaller first, missing last), `blocks` | many entries; two switches the user combines: Groups (a folding heading per group, else one list with chips per group) and Detail (the chosen entry beside the list, under it when narrow; else it opens under its row, one at a time); filter field (Ctrl+F), sort menu ("Active first" by the order of `groups`, the program's `sorts`, A–Z) – all worked out in the renderer, nothing is sent; rows do not reorder while the mouse is over them; an entry that is new or changed its group is marked for a moment (added 2026-11-18, section 11) |

A `section` may carry `"open": false` – it starts folded (missing = open, as before 2026-11-18). A `slider` or
`entry` whose label and control do not fit side by side puts the label above.

Unknown `type` → a line "This part of the page needs a newer Toolbox". Blocks still to come (4.5) get their fields
here when they are built: `combo`, `tabs`, `table`, `graph`, `canvas`.

### 15.3 `program.json`

| Field | Type | Kinds | Notes |
|---|---|---|---|
| `id` | string `[a-z0-9_]+` | all | pipe, task, autostart, job names derive from it (4.1) |
| `name`, `description`, `category` | text | all | text = a string or `{"de": "…", "en": "…"}`; a missing language falls back to `en`, then to the first one |
| `version` | string | all | |
| `exe` | path relative to the manifest | external exe, kit program | what Start/Play runs |
| `run` | path of a `.ps1` relative to the manifest | script | started hidden by the agent: `powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File <run>`, in its job (4.2), in the manifest's folder. Other script types are refused with a message. |
| `launch` | URL or path | script | opened after `run` was started (Rayman: `uplay://…`) |
| `options` | `{"<name>": {"values": [...], "default": …, "label": text, "value_labels": {"<value>": text}}}` | script | shown as `choice` blocks; written to the script's `data\settings.json` as `{"<name>": value}`, which the script reads (a script still in the classic Toolbox: its `data\<id>.json`, carried over once when the script moves – M2) |
| `hint` | text | script | explanation above the options |
| `button` | text | all | label of the start button (default "Start"; Rayman and Last Bastion: "Play") |
| `protocol` | number | kit program | newest protocol it speaks |
| `background` | bool | kit program | has a background role (tray, "quit all") |
| `repository` | `"owner/name"` | all, optional | updates (section 9) |
| `update_key` | base64, 32 bytes | all with `repository` | Ed25519 public key (15.6) |

Kit programs never write it by hand (15.4). Example, Rayman after M2:

```json
| `list` | `key` (string, tells lists apart), `items`, `groups` (list of `{"id","label","open"}`, in their order), `sorts` (list of `{"id","label"}`), `add` (`{"label","blocks"}`, optional), `empty` (text while there is no item). An item: `id` (stable), `title`, `subtitle`, `state` (`ok`/`warn`/`bad`/`unknown`) and `text` (the dot and what it means), `group`, `tags` (list of short texts), `button` (`{"label","action"}`, optional), `order` (`{"sort id": number}`, smaller first, missing last), `blocks` | many entries; two switches the user combines: Groups (a folding heading per group, else one list with chips per group) and Detail (the chosen entry beside the list, under it when narrow; else it opens under its row, one at a time); filter field (Ctrl+F), sort menu ("Active first" by the order of `groups`, the program's `sorts`, A–Z) – all worked out in the renderer, nothing is sent; rows do not reorder while the mouse is over them; an entry that is new or changed its group is marked for a moment (added 2026-11-18, section 11) |

A `section` may carry `"open": false` – it starts folded (missing = open, as before 2026-11-18). A `slider` or
`entry` whose label and control do not fit side by side puts the label above.
{
  "id": "rayman_legends",
  "name": "Rayman Legends",
  "version": "1.1",
  "category": { "de": "Spiele", "en": "Games" },
  "description": { "de": "Startet Rayman Legends rahmenlos …", "en": "Starts Rayman Legends borderless …" },
  "run": "rayman_borderless.ps1",
  "launch": "uplay://launch/410/0",
  "button": { "de": "Spielen", "en": "Play" },
  "options": {
    "Pos": {
      "values": ["Left", "Center", "Right"],
      "default": "Center",
      "label": { "de": "Position", "en": "Position" },
      "value_labels": { "Left": { "de": "Links", "en": "Left" }, "Center": { "de": "Mitte", "en": "Center" } }
    }
  }
}
```

### 15.4 `[package.metadata.toolbox]` (kit programs)

```toml
[package]
name = "hardware-monitor"
version = "2.0.0"                                   # -> "version"
description = "CPU and GPU temperature …"           # -> "description" (en), unless given below
repository = "https://github.com/Windows-Toolbox/Hardware-Monitor"   # -> "repository": "Windows-Toolbox/Hardware-Monitor"

[package.metadata.toolbox]
id = "hardware_monitor"
exe = "HardwareMonitor.exe"                         # the name build.ps1 gives the exe in dist\
category = { de = "Allgemein", en = "General" }
background = true
name = { de = "Hardware-Monitor", en = "Hardware Monitor" }
description = { de = "…", en = "…" }                # optional, overrides [package].description
button = { de = "Spielen", en = "Play" }            # optional: label of the start button (default "Start")
icon = "assets/icon.ico"                            # optional: this is the default; without the file, no icon
dist = "dist"                                       # optional: this is the default; where build.ps1 installs
```

The kit's build helper (`kit::build::manifest()` in `build.rs`) reads this (through `cargo metadata`, so the kit
needs no TOML reader), adds `protocol` and – for a program with a GitHub `repository` – `update_key` (the kit's
`UPDATE_KEY`, unless `[package.metadata.toolbox]` names an own `update_key`), embeds the manifest in the
exe and writes it next to the build output as `<package>.program.json` (several programs may share one build
folder); `build.ps1` copies it into `dist\` as `program.json`. The same build script puts the resource with name,
icon and version into the exe, for Task Manager and the taskbar.

### 15.5 `programs.json` ("Get programs")

In this repo's root, with `programs.json.sig` next to it (15.6). Loaded from
`https://raw.githubusercontent.com/Windows-Toolbox/Toolbox/main/programs.json` only when the list is opened.

```json
{
  "programs": [
    {
      "id": "discord_plus",
      "name": { "de": "Discord Plus", "en": "Discord Plus" },
      "description": { "de": "…", "en": "…" },
      "repository": "Windows-Toolbox/Discord-Plus",
      "update_key": "<base64>"
    }
  ]
}
```

### 15.6 Signed releases

- **Algorithm**: Ed25519 (`ed25519-dalek`). Signed is the zip file's exact bytes.
- **Private key**: 32-byte seed, base64, in `%USERPROFILE%\.windows-toolbox\release-key` – outside every repo.
  Backed up by hand. A small binary `sign` in this repo's workspace (`sign\`) creates the key once and signs files;
  `release.ps1` of every repo calls it: `sign key` shows the public key (and creates the private one the first
  time – an existing but unreadable key file is an error, never a reason for a new key), `sign pack <dist> <zip>`
  packs the folder without `data\` and writes `<zip>.sig`, `sign file <file>` writes `<file>.sig` (for
  `programs.json`). The key was created on 2026-11-16; its public half is `kit::UPDATE_KEY`.
- **Public key**: base64 of the 32 bytes, `update_key` in the manifest (kit programs: a constant in the kit).
- **Release assets**: `<id>-<version>.zip` and `<id>-<version>.zip.sig` (base64 of the 64-byte signature),
  release tag `v<version>`. The zip holds what `dist\` holds, without `data\`.
- **Check**: an update is installed only if the signature fits the `update_key` of the **installed** version; a
  first install from "Get programs" checks against the key in the signed `programs.json`, which itself is checked
  against the key compiled into the Toolbox. No key rotation: a lost key means one manual download (section 9).

### 15.7 Kit modules – public functions

Modules marked *M1* exist in `kit\src\` in this form; the others are fixed when they are built and written back
here.

| Module | Public functions |
|---|---|
| `lib` *(M1)* | `PROTOCOL` · `UPDATE_KEY` (15.6) · `Meta { id, version, dist, manifest }` · `meta!()` – the program's facts from its `Cargo.toml`, through its build script |
| `app` *(M1)* | `trait Program { type Settings; const META; fn page(&self, &mut page::Builder, &Settings); fn status(&self, running: bool) -> Status; fn tray(&self) -> Tray; fn applied(&mut self, &Settings); fn action(&mut self, &str, &mut Settings) -> Option<String> – may change settings, which are then applied and saved like after a `set`; fn show(&mut self); fn shutdown(&mut self) }` · `run(program, worker)` – role, single instance, pipe, settings, delayed saving, the program's **own tray icon** while its workers run and no Toolbox carries its menu for 1.5 s (section 6: the line, its menu, Settings, Quit), `autostart::repair` and `update::clean_up` at start · `Runtime::{changed, running, wait_running, with, update, look}` · `Runtime::stop()` – the worker ends its work itself (a game whose window was closed): workers off, and the program ends once nobody looks at its page (M5) · `Runtime::sleep(time)` – a worker's pause that a `set`, an action, Start or Stop end at once · `open_in_toolbox(id)` · `init(&meta)` – first thing in every exe: application id, data folder, panic → log · `data_dir()` · `program_dir()` · `log(msg)` · `message_box(title, text, error)` · `Look { language, theme, zoom }` · `WORKERS_OFF` · `data_dir_variable(id)` for tests |
| `settings` *(M1)* | `Settings::<T>::load(path)` – missing fields default, invalid ones fall back one by one, unknown ones kept · `.value` · `.existed` · `.save()` (written aside, then renamed) · `extra(key)` / `set_extra(key, value)` – a value next to the struct's fields |
| `page` *(M1)* | `Page { blocks }` · `Block` (15.2) · `Builder::{section, text, status, switch, slider, choice, choice_marked, entry, secret, buttons, finish}` · `apply_set(&settings, key, value) -> Result<T, String>` · `lookup(&value, key)` – a key may be a path (`games.0.zone`, 15.2) · `check_keys::<Settings>(page)` for the test, `check_keys_with(&settings, page)` for a page whose keys point into a list |
| `proto` *(M1)* | `Command`, `State`, `Status`, `Health`, `Tray`, `MenuEntry`, `Live`, `ToolboxHello`, `Want`, `AgentCommand`, `PROTOCOL` – the messages of 15.1 as serde types |
| `ipc` *(M1)* | `pipe_name(id)` · `Server::bind(id)` (`AlreadyExists` = runs already) · `Server::accept()` · `connect(id)` · `connect_wait(id, timeout)` · `exists(id)` · `Conn::{send, send_line, read, read_line, close}` · `TOOLBOX` · `sandbox()` · `legacy::{listening(port), command(port, text, timeout)}` – the Python programs' ports, until the last one is ported |
| `ui` *(M1)* | `theme(ctx, theme, zoom)` · `THEMES` · `PageView::show(ui, &Page) -> Vec<Command>` · `Toast::{show, ui}` · `link_target(&response, target)` / `hovered_target(ctx)` – the bottom bar · `viewport(title)` – how every window starts out · `use_program_icon()` · `no_window(title, error)` – the message when OpenGL is missing · `heading(ui, text)` · `section(ui, title, salt, add)` · `hint(ui, text)` · `bar_button(ui, text, chosen)` – the editor's building blocks · `ACCENT`, `GOOD`, `WARN`, `BAD`, `accent()`, `good()`, `warn()`, `bad()`, `muted()`, `health_color()` |
| `tray` *(M1)* | `create(handler) -> Waker` · `run_loop()` · `Tray::{set_icon(uid, Icon, tip), remove_icon, quit}` – setting what an icon shows already costs nothing · `Icon::{Program, Text(text, color)}` · `program_icon(size)` · `text_icon(text, color)` · `Event`, `Item` · `forget(uid)` – removes an icon at once from any thread, for a process about to end (otherwise it stays until the mouse passes over it) |
| `process` *(M1)* | `Job::{create, open, spawn, active_processes, terminate}` · `job_name(id)` · `spawn(exe, args)` – without a console window · `open(target)` – a file, a folder, an address · `copy_text(text)` – the clipboard, for a page's "Copy" button · `run_elevated(exe)` · `wait_exit(pid, timeout)` · `running(exe) -> pids` · `quote(arg)`. Still to come: `run_task(name)`, `efficiency_mode()` (M4) |
| `i18n` *(M1)* | `t!("id")`, `t!("id", name = value)` · `texts!()` – loads the program's `assets\i18n.json` · `LANGUAGES` · `set_language(lang)` · `language()` · `system_language()` · `check_ids(crate_dir)` for the test |
| `manifest` *(M1)* | `Manifest::read(folder)` (`dist\program.json`, else `program.json`) · `Manifest::find(folder)` – the folder or one level below it · `Manifest::kind() -> Script/Kit/Exe/Classic` · `Text::{get(lang), key()}` · `ScriptOption` |
| `build` *(M1)* | `build::manifest()` for `build.rs` (15.4): `program.json`, the facts for `meta!()`, and the resource with icon, name and version – written by hand as a `.res` file, so no resource compiler has to be installed |
| `autostart` *(M3, built ahead)* | `read(id) -> Entry { state, target }` · `state(id) -> Off/On/DisabledByWindows` – also reads Windows' "switched off" note · `set(id, Some(exe) / None)` – `Some` switches a disabled entry back on · `repair(id, exe)` – re-points an entry whose exe no longer exists (moved folder), leaves one whose exe exists alone · `take_over(old_name, id, exe)` – the Python programs' entries · `ARGUMENT` (`--autostart`: started by Windows, no window) · direct registry calls in HKCU, no PowerShell |
| `http` *(M3, built ahead)* | `get(url, headers, limit)` · `post_form(url, fields, limit)` · `Response::{status, body, ok(), text()}` · `encode(s)` – through WinHTTP (part of Windows: no crate, the PC's proxy and certificates); `https://` only, fixed user agent `windows-toolbox`, nothing about the user or the PC sent along |
| `update` *(M3, built ahead)* | `latest(repository, id) -> Option<Release { version, zip, signature }>` · `download(release, key)` – checks the signature before anything else · `install(zip, folder, id, newer_than) -> Manifest` – the manifest inside the signed zip must name `id` and a newer version; names outside the folder or in `data\` refused; unpacked aside first, files in use put aside, a failed swap undone · `clean_up(folder)` – what an update put aside · `developer_copy(home)` – own builds are not updated · `verify`, `compare_versions`, `base64_encode/decode`, `zip::{write, read}` (stored + deflate, `miniz_oxide`) · `KEY` |
| `setup` | `steps![…]` · `Step::{Winget, Download, Copy, Task}` · `pending() -> Vec<Step>` · `run(steps)` |

**Crates used in M0** (looked up 2026-11-15): `eframe` / `egui` 0.37.0 (features in section 7),
`egui_system_font_provider` 0.37.0 (through eframe's `system_fonts`), `windows-sys` 0.61.2, `serde` 1.0.229,
`serde_json` 1.0.154; Rust 1.99.0, edition 2024. Added in M1 (looked up 2026-11-16): `rfd` 0.18.0 – the folder
dialog of **Add program…**; only the Toolbox uses it. Added ahead of M3 (looked up 2026-11-16): `ed25519-dalek`
3.0.0 (signatures; the kit checks, `sign` signs), `miniz_oxide` 0.9.1 (deflate in release zips), `getrandom` 0.4.3
(only `sign`, for the key). No HTTPS crate: `kit::http` uses WinHTTP.
