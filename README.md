# Windows Toolbox

One window and one tray icon for a collection of small Windows programs: start and stop them, see their status,
change their settings, let them start with Windows. Every program lives in a git repo of its own and runs without
the Toolbox as well; the Toolbox lists, starts and shows them – like Steam does with games.

This repo holds the Toolbox, the library the programs share (`toolbox-kit`) and the tools around them. What is
planned next stands in [DESIGN.md](DESIGN.md); this file describes what exists today.

## The programs

| Program | Repo | What it does | Kind |
|---|---|---|---|
| **Hardware Monitor** | [Hardware-Monitor](https://github.com/Windows-Toolbox/Hardware-Monitor) | CPU and GPU temperature in the taskbar, a window with all hardware values, power consumption with costs, fan curves, drive health, privacy mode. The monitor itself is still Python inside its repo; the Rust program connects it to the Toolbox. | kit program |
| **Discord Plus** | [Discord-Plus](https://github.com/Windows-Toolbox/Discord-Plus) | Extensions for Discord – so far the call status: shows "In a call" while you are in a call or voice chat, for friends without access to the server or call as well | kit program |
| **Screen Zones** | [Screen-Zones](https://github.com/Windows-Toolbox/Screen-Zones) | Puts games and programs borderless into a fixed zone of the screen (16:9 left/centre/right, 21:9, halves) and keeps them there – one entry per game, at every start | kit program |
| **Steam Shortcuts** | [Steam-Shortcuts](https://github.com/Windows-Toolbox/Steam-Shortcuts) | Adds games and programs to Steam (so the Steam Controller works in them) and points desktop icons at Steam – and back | kit program |
| **Cube Game** | [Cube-Game](https://github.com/Windows-Toolbox/Cube-Game) | Arena shooter: the game window keeps shrinking, shots against its edges push them back out (WASD + mouse) | kit program |
| **Last Bastion** | [Last-Bastion](https://github.com/Windows-Toolbox/Last-Bastion) | Real-time strategy game; only its `program.json` ties it to the Toolbox | external exe |
| *Script example* | `template\script\` here | The former Rayman Legends script (replaced by Screen Zones), kept as the example of a small script program | script |

Each program's README describes how to set it up, use it and how it works.

## Getting started

**From a release** (from v2.0.0 on): download `toolbox-<version>.zip` from the
[releases](https://github.com/Windows-Toolbox/Toolbox/releases), unpack it into a folder of its own (not inside
*Program Files*) and double-click `Toolbox.exe`. The exe is not signed (that costs money), so Windows SmartScreen
may warn once at the first start: *More info → Run anyway*. Programs then come with one click under **Get
programs …** in the gear; updates with **Update** – both checked against the release key.

**From source** (needs [Rust](https://rustup.rs)):

```
git clone https://github.com/Windows-Toolbox/Toolbox.git
cd Toolbox
.\build.ps1
```

`build.ps1` builds the release version and installs it into `dist\` – that folder *is* the installed Toolbox:
`dist\Toolbox.exe`, `dist\program.json`, and `dist\data\` with your settings (never touched by a build). It also
works while the Toolbox is running: the running exe is renamed aside and the new one starts next time.

Then start `dist\Toolbox.exe` and add programs with **Add program…** (in the gear). A program is any folder with a
`program.json` – your clone of its repo after its own `build.ps1`, or its unpacked release zip. Choosing a folder
that holds several programs (e.g. `E:\Software\Windows Toolbox\`) adds all of them at once.

Nothing needs to be installed besides: no Python, no runtime, no setup program. The Visual C++ runtime is linked
into every exe.

## Using the Toolbox

**The window**: category tabs at the top, the programs of the tab on the left, the chosen program's page on the
right. Around each program's own page the Toolbox adds the same frame for all of them:

- status (running, not running, crashed – a program whose connection broke off without saying goodbye) and the
  program's own status line,
- **Start · Stop · Restart**,
- **Start with Windows** for programs that keep running in the background – off until switched on. If Windows has
  switched the entry off (Task Manager → Startup) or another copy of the program starts with Windows, the page says
  so; switching on makes Windows start this copy,
- **Open folder**, **Open log**, **README**, **Remove from the list** (the folder stays untouched),
- **Update** for a program whose manifest names its repository: asks GitHub, loads a newer release only, checks its
  signature, swaps the files (`data\` stays as it is) and starts the program again if it ran. A copy you build
  yourself shows "built here – run build.ps1" instead.

A kit program's page shows its settings; a change takes effect in the running program at once. A stopped program
can be configured as well: the Toolbox starts it without its work, only to show the page, and it ends again when
the page is left. A script's page shows its hint and its options.

**The tray icon**: one icon for everything. A left click opens the window; the right-click menu lists every program
with its status and its own entries (Start, Stop, Restart, Settings, whatever the program adds), and at the end
**Quit ▸ Only the Toolbox · Toolbox and all programs · Restart the Toolbox**. Programs have no icon of their own while
the Toolbox runs; without the Toolbox a background program shows its own icon. Windows 11 hides new icons in the
overflow (`^`) – the gear explains how to show it and has a button for the Windows settings page.

**The gear**: colours (dark, light, like Windows), language (German, English), size (also Ctrl + mouse wheel), the
list of programs with **Get programs …** (the signed list in this repo, `programs.json`, loaded from GitHub when it
is opened; "Get" installs a program into `programs\<id>\` next to `Toolbox.exe`), **Add program…** and **Update
all**, the tray icon, **Update** of the Toolbox itself, **Restart** and **Quit**. Language, colours and size are
handed to every running kit program, so their own windows look the same. Closing the window ends nothing – the
Toolbox and the programs keep running.

**Two copies of one program**: a program is known by its `id`, and only one copy of it can be on the list. Adding a
second one (your own build next to a release) asks which of the two to keep.

**Your data**: `dist\data\settings.json` (look, language, the list of program folders) and `dist\data\log.txt`. The
folders are remembered relative to the Toolbox where possible, so moving everything together keeps the list valid.
Every program keeps its own settings in its own `dist\data\`.

## Three kinds of programs

| Kind | What you write | What the Toolbox can do |
|---|---|---|
| **Script** | `program.json` with `run` + a `.ps1` | start it without a window, show "running", stop it with everything it started, options on its page |
| **External exe** | `program.json` with `exe`, in the other program's repo | start it, show "running" (by its process), open its folder |
| **Kit program** | a Rust program using `toolbox-kit` | all of the above + its settings page inside the Toolbox, live status, its menu under the tray icon, a clean stop, language and look |

All fields of `program.json`: [DESIGN.md 15.3](DESIGN.md#153-programjson).

### A small script

A PowerShell script becomes a program with its own page without a line of UI code. It gets a folder of its own
(ideally a repo of its own) holding only `program.json` and the script. The example is `template\script\` – copy the
folder and add it in the Toolbox to try it:

```json
{
  "id": "rayman_legends",
  "name": "Rayman Legends",
  "version": "1.1",
  "category": { "de": "Spiele", "en": "Games" },
  "description": { "de": "Kurze Beschreibung", "en": "Short description" },
  "run": "rayman_borderless.ps1",
  "launch": "uplay://launch/410/0",
  "button": { "de": "Spielen", "en": "Play" },
  "hint": { "de": "Was vorher im Spiel einzustellen ist …", "en": "What to set in the game first …" },
  "options": {
    "Pos": {
      "values": ["Left", "Center", "Right"], "default": "Center",
      "label": { "de": "Position", "en": "Position" },
      "value_labels": { "Left": { "de": "Links", "en": "Left" } }
    }
  }
}
```

- `run`: the `.ps1` the Toolbox starts **without a window**, in the script's folder. Stop ends it together with
  everything it started.
- `launch` (optional): opened after the script – a game (`uplay://…`, `steam://rungameid/…`), a web page or a
  program.
- Texts are a string or one per language (`{ "de": …, "en": … }`). `hint` stands on the page above the options – the
  place for instructions. `button` labels the start button.
- `options` (optional): one choice per option on the page. The Toolbox saves them in `data\settings.json` in the
  script's folder (`{"Pos": "Left"}`); the script reads them there – at its start and, if a change should take
  effect at once, while it runs (example: `Update-Pos` in `rayman_borderless.ps1`). `data\` belongs into the
  `.gitignore`.

### A kit program

A kit program is a Rust crate with `toolbox-kit` as a dependency:

```toml
kit = { package = "toolbox-kit", git = "https://github.com/Windows-Toolbox/Toolbox" }
```

Its manifest is not written by hand: name, version and description come from `[package]` of its `Cargo.toml`, the
rest from `[package.metadata.toolbox]` ([DESIGN.md 15.4](DESIGN.md#154-packagemetadatatoolbox-kit-programs)); the
kit's build helper writes `program.json` and compiles name, icon and version into the exe. Settings are one struct,
the page is described in a few lines, and the kit does the rest: loading and saving, the pipe to the Toolbox, the
tray, autostart, texts in several languages. A new program starts as a copy of `template\kit-program\` – its
README says which names to change; `demo\` is the kit's own test program, the programs above are the bigger ones.
How it works: [DESIGN.md 5](DESIGN.md#5-toolbox-kit--the-shared-library), the formats in
[DESIGN.md 15](DESIGN.md#15-formats).

## This repo

```
Toolbox/
├─ Cargo.toml        the workspace: one command builds everything below
├─ build.ps1         release build of the Toolbox → dist\
├─ release.ps1       a signed release of the Toolbox on GitHub (RELEASE.md: its text)
├─ programs.json     the list of "Get programs", with its signature programs.json.sig
├─ kit/              toolbox-kit, the library every program and the Toolbox share
├─ toolbox/          the Toolbox (src\: agent, window, list of programs; assets\: icon and texts)
├─ demo/             Kit Demo: the smallest kit program – the kit's tests talk to it
├─ sign/             the developer's signing tool behind every release.ps1
├─ template/
│  ├─ kit-program/   the template of a new kit program
│  └─ script/        the example of a script program
└─ dist/             (ignored) the installed Toolbox and its data\
```

`Toolbox.exe` runs in two roles: the **agent** – always running, the tray icon, the list of programs, the pipe
`\\.\pipe\windows-toolbox.toolbox`; it sleeps in the Windows message loop until something happens – and the
**window**, a second process that exists only while the window is open and gives all its memory back when it
closes.

**All repos in one folder**: with the programs cloned next to this repo (`E:\Software\Windows Toolbox\<program>\`),
one small file `Windows Toolbox\.cargo\config.toml` points every program at the local `kit\` instead of the git
address – a change in the kit shows up in every program at its next build, without committing:

```toml
[patch."https://github.com/Windows-Toolbox/Toolbox"]
toolbox-kit = { path = "Toolbox/kit" }
```

Someone who clones a single program does not have that file and gets the kit from GitHub.

## Testing

```
cargo test --workspace
```

Nothing pops up: pages and tray menus are tested as data, the agent runs in a sandbox (its own pipe names, no tray
icon, its own data folder) and is talked to the way the window does, and every page of the window is drawn into
memory. Details: [DESIGN.md 13](DESIGN.md#13-testing-without-windows).

## Releases

Programs publish signed releases with their own `release.ps1`: it builds, zips `dist\` without `data\`, signs the zip
with `sign` and publishes it on GitHub. The private key lies in `%USERPROFILE%\.windows-toolbox\release-key`, outside
every repo – keep it safe and backed up; whoever has it can release updates every installed program accepts. The
public key is compiled into the kit, so an update is only installed if it is signed with the key of the installed
version ([DESIGN.md 15.6](DESIGN.md#156-signed-releases)).

The `.exe` files themselves are not signed (signing costs money): Windows SmartScreen may warn once at the first
start of a downloaded program – *More info → Run anyway*.

## License

[MIT](LICENSE).
