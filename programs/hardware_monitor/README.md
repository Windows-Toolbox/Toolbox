# Hardware Monitor

Shows CPU and GPU temperature as two numbers in the notification area of the taskbar (without a GPU temperature,
e.g. with Intel onboard graphics, only the CPU). A click on one of the icons opens a
detail window with almost all hardware values in tabs (CPU, GPU, RAM, motherboard, drives, network, battery) and the
tab **Hardware data**: manufacturers, models, vendor and product IDs, serial numbers and drivers of all components.
On top: the **power consumption** of the whole PC with costs and a chart (tab *Power*), **fan curves** for graphics
card and case fans like in MSI Afterburner (tab *Fans*), the **drive health** like CrystalDiskInfo (tab *Drive
health*) and a **privacy mode** that hides serial numbers and the like for screenshots.
Runs on AMD and Intel systems, desktop and laptop alike; tabs for devices the PC doesn't have (e.g. motherboard
sensors on a laptop, a battery on a desktop) are hidden.
The sensors are read by [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) (LHM)
via pythonnet.

Runs as its own program **with admin rights** (otherwise CPU and motherboard values are missing). It is operated and
configured via its page in the Toolbox (*General → Hardware Monitor*) or by right-clicking an icon.

## Files

| File | Content |
|---|---|
| `hardware_monitor.py` | the program: reading sensors, tray icons, remote control by the Toolbox |
| `detail_window_content.py` | **what** the detail window shows (tabs, values) – this is where you adjust the window |
| `detail_window.py` | builds the window from `detail_window_content.py` (colors, layout, traffic-light colors) |
| `hardware_data.py` | reads the fixed data for the tab "Hardware data" via WMI; `python hardware_data.py` prints it |
| `energy.py` | power consumption: estimate, log per hour, add up (no window code – the Toolbox page uses it too) |
| `energy_view.py` | the tab "Power" in the detail window: tiles and bar chart |
| `fans.py` | fan control: curves, safety rules, handing back (no window code) |
| `fan_view.py` | the tab "Fans" in the detail window: list of fans and the curve editor |
| `drive_view.py` | the tab "Drive health" in the detail window: status and SMART table like CrystalDiskInfo |
| `hardware_monitor_gui.py` | page in the Toolbox: status, start/stop, autostart, settings |
| `hardware_monitor_settings.py` | defaults, allowed values, loading and saving, number format – shared by program and page |
| `i18n.json` | all texts in German and English – one block per ID, one line per language (see *Languages*) |
| `program.json` | manifest: name, tab, description, version, script, starter, port, task, download of LibreHardwareMonitor |
| `HardwareMonitor.exe` | starter (created by `Setup.exe`, not in the repo) – starts `hardware_monitor.py` without arguments |
| `lhm/` | LibreHardwareMonitor (not in the repo, see below) |
| `data/hardware_monitor.json` (in the main folder) | settings (created on the first save, not in the repo) |
| `data/logs/hardware_monitor.log` (in the main folder) | errors and notes – `pythonw` has no console (not in the repo) |
| `data/hardware_monitor_energy.json` (in the main folder) | logged power consumption per hour (only with logging on, not in the repo) |

## Setup

**`Setup.exe` in the repo folder** does everything (see the main README): its own Python in `runtime\`, the packages
(`pythonnet`, `pystray`, `pillow`), the starter `HardwareMonitor.exe`, LibreHardwareMonitor 0.9.6 into `lhm\`, PawnIO
and the scheduled task `HardwareMonitor`. By hand it would go like this:

1. **Python packages**: `pip install -r requirements.txt` in the repo folder (pinned versions, tested with Python 3.14).
2. **PawnIO** (driver for CPU and motherboard sensors), once in an admin terminal:
   ```
   winget install namazso.PawnIO
   ```
3. **LibreHardwareMonitor**: extract `LibreHardwareMonitor.zip` (not the .NET 10 variant) from the
   [GitHub releases](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases) into `lhm\` in
   this folder.
4. **Autostart**: click *Set up task* in the Toolbox. That creates the scheduled task `HardwareMonitor` (name from
   `"task"` in `program.json`), which starts the program at sign-in with admin rights – without a UAC prompt. It is
   created disabled (Setup.exe does the same); the switch *Start automatically at sign-in* enables it. If it is
   disabled or missing, *Start* in the Toolbox starts the program directly, and Windows asks every time.
   Setting it up again (button, Setup.exe, update) leaves the switch as it was.

**What it runs with:** task and *Start* call the starter `HardwareMonitor.exe` in this folder (without
arguments). It starts `runtime\HardwareMonitor.exe` (`pythonw.exe` of the project Python with its own file
description and icon, created by `setup\make_exe.py`) with `hardware_monitor.py` and exits – Task Manager then
shows "Hardware Monitor" with the Toolbox icon instead of "Python". If the starter is missing, the Toolbox shows a hint
to run `Setup.exe`. While the detail window is closed, the monitor appears in Task Manager under "Background
processes" instead of "Apps" – Windows does that with every program without a visible window.

## Starting and checking

```
pythonw hardware_monitor.py          # tray icons, without a console window
python  hardware_monitor.py --list   # list all sensors with device, type, name and value
```

If `--list` in an admin terminal shows real temperatures for the CPU, everything works. Only one instance runs at a
time; a second start exits right away.

## Usage

- **Left-click** an icon: open/close the detail window. The window remembers its size and position.
- **Status at the top right**: when the window opens, the values of the selected tab are measured right away; until
  they are there it says "Loading data …", then "● Live" until it's closed. On a tab change the devices of the new
  tab are measured right away, and the list briefly shows "Loading sensor data …". While the window is closed or
  minimized, only CPU and GPU are measured for the icons – that's why the window loads briefly when it opens. Right
  after the program starts it takes longer (~10 s), because the remaining hardware is still being loaded (mainly the
  RAM).
- **⚙ to the right of it**: opens the settings – the Hardware Monitor's page in the Toolbox. If the Toolbox isn't
  running, it is started, as a normal program (via `explorer.exe`), even though the monitor itself has admin rights.
- **Click on a value or device name** copies it to the clipboard, e.g. "2786" for the product ID or
  "G.Skill F4-3200C16-8GTZRX" for a memory module – but only directly on the text (the hand as mouse cursor shows it).
  A click on the label in front of it ("Product ID") or into the empty space next to it only selects the row. The
  footer briefly confirms the copy; when hovering it shows the full value.
- **Copying several rows**: select them – drag across them with the mouse button pressed, Ctrl+click (add single
  ones), Shift+click (up to here) or Ctrl+A (all) – and copy with Ctrl+C, e.g. "Vendor ID: 10DE" and
  "Product ID: 2786" below each other. Headings only come along with their name, indented like in the list.
  The same is in the list's right-click menu; Esc or a click into empty space clears the selection.
- **Tab "Hardware data"** has two sub-tabs: "Overview" only shows the model names (identical devices combined,
  e.g. "2× G.Skill …"), "Details" all fields (manufacturer, IDs, serial numbers, drivers …). What is copied is always
  what is shown right now: "⧉ Copy" on the right of every category row (Processor, Graphics card, …) the category,
  "Copy all" at the top all categories – in the overview one line per category, below it every device with a tab
  in front; in the details as text with computer name and date, e.g. for pasting into a forum.
- **Privacy mode** (☐/☑ left of "● Live", in the right-click menu of the icons and on the Toolbox page): hides
  serial numbers, MAC addresses, the processor ID and the computer name – in the list, in the footer and when
  copying they appear as `████████`, e.g. for screenshots or forum posts. The window title then only says "Hardware
  Monitor" instead of the program path (which can contain the Windows user name). The real values stay in memory;
  only display and copying replace them (`SENSITIVE_FIELDS` and `shown()` in `hardware_data.py`). The switch is
  saved and still applies after a restart.
- **Tab "Power"**: what the whole PC draws right now (estimated, see *Power consumption*), today and this month in
  kWh with costs, and how long Windows has been running ("PC on for", like "Up time" in Task Manager). Below it a
  bar chart of the logged consumption – sub-tabs "24 hours" (per hour), "30 days" (per day), "12 months" (per month);
  hovering over a bar shows period, energy, cost and measured time in the footer. "Copy table" copies the bars
  tab-separated (e.g. for Excel).
- **Tab "Fans"**: fan curves like in MSI Afterburner – see *Fan control*.
- **Tab "Drive health"**: like CrystalDiskInfo – see *Drive health*.
- **Tab bar**: if the window is too narrow for all tabs, they wrap into a second row (own buttons instead of the
  tabs of `ttk.Notebook`, which would squeeze the titles until they're unreadable).
- **Right-click an icon**: show details, privacy mode, refresh rate of the icons (1–10 s) and of the window
  (0.5–5 s), quit.
- **Number color**: green, from `…_warn_temp` yellow, from `…_hot_temp` red – separately for CPU (default 75 / 90 °C)
  and GPU (70 / 83 °C). In the detail window motherboard and drives follow the CPU limits.

## Settings

Saved in `data\hardware_monitor.json`. The Toolbox writes the file and then sends `reload`; the running program
takes over the values right away. Missing or invalid values get the default.

| Key | Default | Meaning |
|---|---|---|
| `icon_interval` | `2` | seconds between measurements for the icons (1, 2, 5, 10) |
| `popup_interval` | `1` | seconds between measurements while the window is open (0.5, 1, 2, 5) |
| `icon_order` | `["cpu", "gpu"]` | order of the icons – only takes effect after restarting the program |
| `cpu_sensor` / `gpu_sensor` | `"auto"` | sensor in the icon; `auto` takes the first existing one from `CPU_SENSORS` or `GPU_SENSORS` |
| `cpu_warn_temp` / `cpu_hot_temp` | `75` / `90` | CPU: yellow or red from here (also motherboard, drives) |
| `gpu_warn_temp` / `gpu_hot_temp` | `70` / `83` | GPU: yellow or red from here |
| `efficiency_mode` | `"auto"` | Windows 11 efficiency mode: `auto` = only while the window is closed, `on` = always, `off` = never |
| `window` | `""` | size and position of the detail window (set by the window itself) |
| `privacy_mode` | `false` | privacy mode: hide serial numbers, MAC addresses, processor ID and computer name |
| `energy_logging` | `false` | log the power consumption (switch on the Toolbox page) |
| `energy_price` / `energy_currency` | `0.35` / `"€"` | price per kWh (an average – no dynamic prices) and its currency |
| `energy_other_watts` | `40` | estimated power of everything except CPU and graphics card (0–300 W) |
| `energy_psu_efficiency` | `87` | efficiency of the power supply in % (50–100) |
| `fan_control` | `{}` | per fan (LHM ID): `mode` (`auto`/`curve`/`fixed`), `source` (`gpu`/`cpu`/`max`), `curve` (`[[°C, %], …]`, 2–8 points), `fixed` (%) – missing = `auto` |

Defaults, allowed values and the check when loading are in `hardware_monitor_settings.py` – monitor and Toolbox
page both use this file. The Toolbox page only writes the values that were changed on it – the monitor changes some
itself (rates and privacy mode in the right-click menu, window position), and an older state shown on the page must
not overwrite them. Until November 2026 some keys had German names (`effizienzmodus` with `an`/`aus`);
`load_settings()` takes them over once.

## Adjusting the detail window

What the window shows is in `detail_window_content.py` – tab by tab, one row per value:

```python
Tab("tab.gpu",
    Group("gpu",
          Value("GPU Core", type="Temperature"),
          Value("GPU Hot Spot"),
          Value("GPU Fan *", type="Fan")))       # * = any text, here: all GPU fans
```

- Labels (tab titles, `Section`, `title=`, `text=`) are IDs from `i18n.json` like `"tab.gpu"` – that way they read
  correctly in every language. A custom text without ID works too; it then reads the same in every language.
- Devices for `Group(...)`: `cpu`, `gpu`, `ram`, `motherboard`, `drive` (applies to every drive separately),
  `network`, `battery`, `other`. `name=` limits a group to devices with a matching name (e.g. `"Intel*"`).
- `type=` is only needed if the same name exists several times – the sensor type as `--list` shows it
  (`Temperature`, `Load`, `Clock`, `Power`, `Voltage`, `Fan`, `Control` …); `text=` sets a custom label; `exclude=`
  leaves out names that don't belong despite `*` (`Value("CPU Core #*", exclude="*Distance*")`).
- `Section("section.fans", Value(...), ...)` in a group is a collapsible sub-item; with `one_row=True` values with the
  same name are in one row (Fan #1: speed, then the control as a bar with %; if a driven fan doesn't spin, it says
  "stopped" in gray). Collapsed areas stay collapsed until the program quits.
- The names depend on the hardware (AMD: `Core (Tctl/Tdie)`, Intel: `CPU Package`). Whatever doesn't exist on the PC
  isn't shown – that's why the names for both are in the file side by side.
- The exact names are shown by `--list`. The former tab "All" (all sensors) can be commented back in in
  `detail_window_content.py`.
- Saved changes appear the next time the window opens, without a restart.

## Languages

All texts – Toolbox page, right-click menu, detail window, explanations, hardware data – are in `i18n.json`: one
block per ID, in it every language on its own line:

```json
"page.status.running": {
  "de": "Läuft",
  "en": "Running"
},
```

The code only contains the ID: `t("page.status.running")`; placeholders like `{task}` are filled by `t("…", task=…)`.
If a translation is missing, the English text comes, if that is missing too, the ID itself (once per ID in the log) –
so nothing crashes. New texts are only added in English for now; the German interface then shows them in English.
New language: add a line in `toolbox\languages.json` (`"fr": "Français"`) and here a line in every block
(`"fr": "…"`). The FR button in the Toolbox and the radio button "Français (FR)" in the setup window are both built
from it, and a Windows in this language preselects it automatically.

Which language applies is in `data\toolbox.json` (`"language"`) – chosen in the setup window or in the Toolbox under
⚙ → *Appearance* (DE / EN). Without an entry the Windows display language applies if it exists (otherwise English). A
click on DE/EN takes effect right away: the Toolbox rebuilds the page and sends the running monitor `reload` – the
right-click menu, tooltips and an open detail window switch right away. Sensor names from LibreHardwareMonitor
("CPU Package") stay English. Numbers and dates follow the language (`num()`, `fmt_date()` in
`hardware_monitor_settings.py`): English 2.5 and 2021-03-03, German 2,5 and 03.03.2021.

## Power consumption

Most power supplies can't report what they draw – only a few "digital" ones with a USB cable to the motherboard
(Corsair HXi/RMi/AXi, some MSI models), and LibreHardwareMonitor reads those (`IsPsuEnabled`). So the power at the
wall socket is **estimated** (`energy.py`):

    (CPU package power + graphics card power + other components) ÷ power supply efficiency

- **CPU**: the sensor `Package` (AMD) or `CPU Package` (Intel) – its device is updated for the icon anyway.
- **Graphics card**: with Nvidia the power of the whole card from `nvml.dll`, otherwise the LHM sensor
  (`GPU Board Power`, `GPU Package`, …). Graphics in the processor (Intel UHD/Iris, AMD "Radeon(TM) Graphics") is
  skipped – its power is already part of the CPU package (`integrated_gpu()`).
- **Other components** (motherboard, RAM, drives, fans, USB devices) and the **efficiency** are fixed values from the
  Toolbox page – about 30–60 W and 85–90 % for a desktop PC. With an energy meter at the socket you can calibrate
  "Other components" until "Now" matches.
- **Laptop on battery**: the battery's discharge rate is what the whole laptop draws – then that measured value is
  used ("Now" without "≈").

With **Log power consumption** switched on (Toolbox page), every measurement (`icon_interval`, with the window open
`popup_interval`) adds watts × seconds to its hour in `data\hardware_monitor_energy.json` – saved every 5 minutes
and when quitting, about 300 KB per year. Only while the monitor runs; gaps longer than 30 s (sleep) don't count.
Costs = kWh × price, calculated when shown. *Delete all saved data* on the Toolbox page asks first, then deletes the
file (and, if the monitor is running, its values in memory via `energy_delete`). *Show chart* opens the detail
window on the tab "Power" (`show_power`). The Toolbox page shows a summary (today, this month, in total) from the
running monitor or, if it isn't running, from the file.

## Fan control

Curves for the graphics card fans and the motherboard fan headers, set in the detail window, tab **Fans** (the
Toolbox page has a button for it and explains the safety rules). LibreHardwareMonitor does the actual controlling:
`sensor.Control.SetSoftware(%)` takes a fan over, `SetDefault()` hands it back. What it can control depends on the
hardware – on the developer's PC the RTX 4060 Ti (both fans together) and the five headers of the motherboard chip
(Nuvoton NCT6779D); the motherboard headers need admin rights (PawnIO), like their sensors.

- **One entry per graphics card** (all its fans follow one curve, like in Afterburner) and **one per motherboard
  header** ("Fan Control #2" belongs to the speed "Fan #2"). The list shows speed, current control value and mode.
- **Modes**: *Auto* (the card or the BIOS controls the fan, as without this program – the default), *Curve*
  (temperature → %), *Fixed* (always the same %). **Temperature**: GPU, CPU or *Highest* of the two – the sensors of
  the taskbar icons.
- **Curve editor**: 20–100 °C × 0–100 %. Drag a point (it stays between its neighbors), double-click to add one
  (up to 8), right-click to remove one (at least 2 stay). *Default curve* restores a quiet starting curve. The white
  line shows the current temperature, the dot the speed the fan gets right now; below it "set" shows what was
  really sent to the fan.
- **Minimum**: some fans can't run slower while controlled – an RTX 4060 Ti doesn't go below 30 % (line in the
  chart). In *Auto* the card can stop its fans completely when idle (0 RPM), which is usually quieter at idle.

Safety (`fans.py`):

- Only fans switched to *Curve* or *Fixed* are touched.
- From the **red limit** of the traffic light (`gpu_hot_temp` / `cpu_hot_temp`, Toolbox page) on, a controlled fan
  runs at **100 %** (shaded red in the editor).
- Without a temperature (sensor gone) or if setting fails, the fan is handed back.
- **Hysteresis** of 3 °C: a fan only slows down again once the temperature has dropped 3 °C below the value at
  which it was last sped up – otherwise it hums up and down with every degree.
- While a fan is controlled, the monitor measures at least every 2 s and sets the speed again every 30 s anyway
  (after sleep the driver may have taken over).
- **Quitting** hands every fan back – also after an error. Only if the monitor is killed (Task Manager, crash) does a
  fan keep its last speed until Windows restarts; the hardware then protects itself by throttling. The file
  `data\hardware_monitor_fans.active` exists while fans are controlled – if it is still there at the next start,
  the tab shows a warning.
- *Reset everything* on the Toolbox page sets all fans back to *Auto*.

## Drive health

The tab **Drive health** shows what CrystalDiskInfo shows: per drive the health status (**Good / Caution / Bad**,
always with the word, not only the color), temperature, remaining life (SSDs) and power-on hours; for the selected
drive firmware, interface, serial number (hidden in privacy mode), size, power-on count, host reads/writes and the
full **SMART table** (ID, attribute, current, worst, threshold, raw values in hex) with a colored dot per attribute.
*Copy* copies the drive's data and table as text.

The values come from **DiskInfoToolkit** – the library LibreHardwareMonitor 0.9.6 reads drives with, a port of
CrystalDiskInfo's logic (`read_drive_health()` in `hardware_monitor.py`). It needs admin rights and is read every
10 s while the tab is shown. The overall status is DiskInfoToolkit's; the dot per attribute follows CrystalDiskInfo's
rules: below its threshold = bad; on hard disks reallocated, pending or uncorrectable sectors (05, C5, C6) with a raw
value above 0 = caution; NVMe: a critical warning = bad, data integrity errors or a spare below its threshold =
caution. NVMe values have no ID of their own and are numbered 01, 02, … like in CrystalDiskInfo. Temperature colors:
the limits an NVMe drive reports itself, otherwise 55/65 °C for SSDs and 45/55 °C for hard disks.

## Hardware data

When shown, the tab reads once via WMI (through pythonnet, `System.Management`) what Windows knows about the
hardware – no sensors, no admin rights needed, under 1 s. After every reopening of the window it is read again (e.g.
for freshly plugged-in USB devices); until then the old data stays.

| Category | Fields |
|---|---|
| Processor | manufacturer, cores/threads, base clock, L3 cache, socket, processor ID (CPUID) |
| Graphics card | kind (integrated/dedicated/external), vendor ID, product ID, board vendor, subsystem ID, revision, hardware ID, driver |
| Motherboard | manufacturer, model, version, serial number, BIOS vendor/version/date |
| Memory | per module: manufacturer, part number, size, type, form factor (DIMM/SO-DIMM), speed, slot, serial number. Soldered LPDDR memory as one entry: total size, form factor "soldered", channels |
| Drives | kind (SSD/HDD), interface (NVMe/SATA/USB), size, serial number, firmware |
| Network | kind (LAN/WLAN/mobile broadband), vendor/product ID, board vendor, hardware ID, MAC address |
| Audio | vendor/product ID, board vendor, hardware ID |
| USB devices | vendor ID (VID), product ID (PID), serial number – one entry per device |

- Vendor names behind the IDs ("10DE (Nvidia)") come from small tables in `hardware_data.py`
  (`PCI_VENDORS`, `USB_VENDORS`). Unknown IDs are shown without a name – add them there if needed.
- Windows often reports the RAM manufacturer only as "Unknown"; then it is derived from the start of the part number
  (`RAM_PART_NUMBERS`) and marked as "(from part number)".
- Virtual devices (VPN adapters, virtual displays and audio devices) are left out.
- With this tab the measuring thread only reads CPU and GPU for the icons.

## Remote control (IPC)

The Toolbox runs without admin rights and therefore can't simply end the program. Instead it listens on
`127.0.0.1:47815` (only reachable from this PC) for one command per connection; the answer is one JSON line:

| Command | Answer / effect |
|---|---|
| `status` | `{"ok": true, "pid": …, "cpu": 52.0, "gpu": 41.0, "order": ["cpu", "gpu"], "icons": [...], "privacy_mode": false}` |
| `reload` | read the settings again |
| `quit` | quit the program |
| `show_power` | open the detail window on the tab "Power" |
| `show_fans` | open the detail window on the tab "Fans" |
| `drive_health` | the last drive health reading (`{"drives": [...]}`) – and read again within the next seconds (for checking without a window) |
| `energy_delete` | delete all logged power consumption (memory and file) |

`status` also contains `"power"` (watts now, or `null` while nothing is measured) and `"energy"` (totals: today,
month, year, all as `[Wh, seconds]`, and `"since"`).

The port is `"port"` in `program.json`; monitor, Toolbox page and `setup.ps1` read it from there.

## Economy

- **Window closed or minimized**: only CPU and GPU are measured. With Nvidia the GPU temperature comes directly from
  `nvml.dll` (part of the driver) – a fraction of the ~90 ms that a complete GPU update via LHM costs. That applies
  to the sensor choice "Automatic" and "GPU Core"; other GPU sensors go via LHM.
- **Only one value per icon**: for the icons only the one temperature sensor is read (`icon_values()`), not all
  ~50 CPU sensors one by one via pythonnet. The sensor is searched once and then queried directly.
- **Windows 11 efficiency mode**: while the window is closed the program runs in efficiency mode (leaf symbol in
  Task Manager: scheduled power-efficiently, lowest priority), with the window open normally. Configurable in the
  Toolbox (*Refresh → Efficiency mode*: Automatic / Always on / Off; the "i" next to it explains the three). If it is
  switched by hand in Task Manager, the program sets its mode again the next time the window opens or closes.
- **No GPU icon without a GPU temperature**: at the start `gpu_has_temperature()` checks whether there is a GPU
  temperature (Nvidia via NVML or any temperature sensor of a GPU in LHM). If not, the GPU icon is left out, and the
  GPU isn't even read for the icons.
- **Hardly any waking up when idle**: the remote control waits for connections without a timeout; while the window
  is closed the interface only checks for clicks on the icon every 250 ms (open: 100 ms).
- **Window open**: only the devices of the selected tab are measured; the icons get the values too.
- **Fast start**: first only CPU and GPU are opened (icons after a fraction of a second), the remaining hardware
  loads in the background. The RAM alone needs ~8 s for that; until then the window shows "Loading sensor data …".
- Icons are only redrawn when the displayed number changes.

## Troubleshooting

- **CPU shows `--` or is missing**: the program runs without admin rights, or PawnIO isn't installed.
- **No GPU icon**: the graphics has no temperature sensor – normal with Intel onboard graphics (e.g. Iris Xe)
  (the log says so too).
- **Task Manager still shows "Python" or the Python logo**: run `Setup.exe` again (it recreates the `.exe` files and
  lets Windows read name and icon again), then close Task Manager completely and open it again – an open one keeps
  what it loaded once.
- **A tab shows "No matching sensors"**: the hardware names its sensors differently; `--list` shows the names,
  which are then added in `detail_window_content.py`.
- **Doesn't start at all**: look at `data\logs\hardware_monitor.log` (Toolbox: *Advanced → Open log*). Often `lhm\`
  is missing or it's the .NET 10 variant of LHM.
- **Toolbox shows "Stopped" although the icons are there**: port 47815 was taken at the start; the program tries
  again every 2 s (the log says so too).
- **Tab "Hardware data" stays empty**: errors of the WMI queries are in the log; `python hardware_data.py` shows
  what Windows delivers.
- **Task starts an old copy**: the Toolbox warns if the task doesn't point to `HardwareMonitor.exe` in this folder
  (e.g. after moving the folder); *Update task* or `Setup.exe` switches it over.
