"""
The detail window of the Hardware Monitor (tkinter + ttk).

WHAT the window shows is in detail_window_content.py – that's the only place you need to change, e.g.:

    Tab("GPU",
        Group("gpu",
              Value("GPU Core", type="Temperature"),
              Value("GPU Fan *", type="Fan")))

This file builds the window from it and normally doesn't need to be touched – like base.py for the Toolbox.
Changes to detail_window_content.py are picked up the next time the window opens (no restart needed).
All texts are in i18n.json – only their ID is here: t("window.…") (see toolbox\\i18n.py).
"""

import ctypes
import fnmatch
import importlib
import json
import logging
import re
import socket
import subprocess
import threading
import time
import tkinter as tk
import tkinter.font as tkfont
from ctypes import wintypes
from pathlib import Path
from tkinter import ttk

import hardware_data
from drive_view import DriveHealthView
from energy_view import EnergyView
from fan_view import FanView
from hardware_monitor_settings import num, t

CONTENT_FILE = Path(__file__).with_name("detail_window_content.py")
ICON_FILE = Path(__file__).resolve().parents[2] / "toolbox" / "program.ico"     # ⚒ on green, like all programs
# Window title: the path of the program, the way Explorer shows its folder. In Task Manager it then appears under
# "Hardware Monitor" and shows which copy is running (e.g. after the folder was moved).
PROGRAM_FILE = Path(__file__).resolve().with_name("hardware_monitor.py")
# Gear at the top right: opens the Hardware Monitor's page in the Toolbox (open_toolbox)
PROGRAM_ID = json.loads(PROGRAM_FILE.with_name("program.json").read_text(encoding="utf-8"))["id"]
TOOLBOX_DIR = Path(__file__).resolve().parents[2]
TOOLBOX_EXE = TOOLBOX_DIR / "Toolbox.exe"            # starter of the Toolbox (created by Setup.exe)
TOOLBOX_PORT = json.loads((TOOLBOX_DIR / "toolbox" / "program.json").read_text(encoding="utf-8"))["port"]


def toolbox_command(command):
    """Sends a command to the running Toolbox (see instance_server in toolbox.py). True if it says "ok"."""
    try:
        with socket.create_connection(("127.0.0.1", TOOLBOX_PORT), timeout=0.5) as conn:
            conn.sendall(f"{command}\n".encode())
            return conn.recv(16).startswith(b"ok")
    except OSError:
        return False


def open_toolbox():
    """Show the Hardware Monitor's page in the Toolbox – blocks for up to 15 s, so call it in the background.
    If the Toolbox is running, "show hardware_monitor" is enough. Otherwise it gets started, via explorer.exe: the
    monitor usually runs with admin rights (task, UAC) – started directly, the Toolbox would have them too.
    explorer.exe starts it as a normal program but doesn't pass on arguments. That's why "show …" is sent
    afterwards, as soon as it answers. True if the Toolbox shows the page."""
    ctypes.windll.user32.AllowSetForegroundWindow(-1)  # ASFW_ANY: the Toolbox may bring its window to the front
    command = f"show {PROGRAM_ID}"
    if toolbox_command(command):
        return True
    if not TOOLBOX_EXE.exists():
        logging.warning("%s is missing – run Setup.exe", TOOLBOX_EXE)
        return False
    try:
        subprocess.Popen(["explorer.exe", str(TOOLBOX_EXE)])
    except OSError:
        logging.exception("Toolbox could not be started")
        return False
    for _ in range(60):
        time.sleep(0.25)
        if toolbox_command(command):
            return True
    logging.warning("Toolbox didn't answer after 15 s")
    return False


# ---------- Building blocks for detail_window_content.py ----------
def label_text(text):
    """Title from detail_window_content.py: an ID from i18n.json (e.g. "tab.gpu") in the chosen language – or a
    custom text without ID, which stays as it is."""
    return t(text) if text in t else text


class Tab:
    """A tab in the window: title (ID from i18n.json or custom text) and the groups in it (or AllSensors() or
    HardwareData())."""

    def __init__(self, title, *groups):
        self.title, self.groups = title, groups


class Group:
    """Values of one device, with the device name as heading.

    device: "cpu", "gpu", "ram", "motherboard", "drive", "network", "battery" or "other"
            (shown in square brackets by `hardware_monitor.py --list`).
            If several devices match (e.g. three drives), the group appears once for each.
            If the PC has none of them (e.g. no battery), the tab is hidden.
    name:   only devices whose name matches (* as wildcard), e.g. name="Total Memory"
    title:  own heading instead of the device name (ID from i18n.json or custom text)
    values: Value(...) rows and Section(...)s (collapsible sub-items, e.g. all fans)
    """

    def __init__(self, device, *values, name="*", title=None):
        self.device, self.values, self.name, self.title = device, values, name, title


class Value:
    """One row in the window.

    sensor:  sensor name as shown by `hardware_monitor.py --list`; * as wildcard (e.g. "Fan #*" = all fans).
    type:    only needed if the name exists several times ("GPU Core" is a temperature, a load and a clock) –
             the sensor type as shown by --list, e.g. "Temperature", "Load", "Clock", "Power", "Fan", "Control".
    text:    own label instead of the sensor name (ID from i18n.json or custom text).
    exclude: sensors that don't belong despite the wildcard, e.g. exclude="*Distance*" for "CPU Core #*".
    """

    def __init__(self, sensor, type=None, text=None, exclude=None):
        self.sensor, self.type, self.text, self.exclude = sensor, type, text, exclude

    def matches(self, s):
        name = s["name"].lower()
        return ((self.type is None or s["type"] == self.type)
                and fnmatch.fnmatchcase(name, self.sensor.lower())
                and not (self.exclude and fnmatch.fnmatchcase(name, self.exclude.lower())))


class Section:
    """A collapsible sub-item in a group, e.g. all fans. Without matching sensors it's left out.

    title:   label of the sub-item (ID from i18n.json or custom text)
    values:  Value(...) rows like in a group
    one_row: values with the same name in ONE row: the first as value, the second as a bar with the percentage
             behind it – e.g. Fan #1 with speed and control. "Control" in the name doesn't count, so
             "Fan Control #1" belongs to "Fan #1".
    """

    def __init__(self, title, *values, one_row=False):
        self.title, self.values, self.one_row = title, values, one_row

    def rows(self, sensors):
        """[(text, sensor)] in display order. With one_row the sensor has the second value under "bar"."""
        rows, seen = [], set()
        for value in self.values:
            for s in sensors:
                if value.matches(s) and s["id"] not in seen:     # matches two patterns (*Fan*, *Pump*) -> only once
                    seen.add(s["id"])
                    rows.append((label_text(value.text) if value.text else s["name"], s))
        if not self.one_row:
            return rows
        combined = {}                                    # name without "Control" -> [text, sensor]; dict keeps the order
        for text, s in rows:
            name = re.sub(r"\s*\bControl\b", "", s["name"])
            if name not in combined:
                combined[name] = [text, s]
            elif "bar" not in combined[name][1]:
                combined[name][1] = {**combined[name][1], "bar": s}      # copy: the snapshot stays unchanged
        return [tuple(row) for row in combined.values()]


class AllSensors:
    """Everything LibreHardwareMonitor finds – sorted by device and sensor type. Good for looking up names."""


class HardwareData:
    """Fixed data of the hardware: manufacturer, model, vendor and product IDs, serial numbers, drivers.
    Read once from Windows when the window opens (hardware_data.py), not measured continuously."""


class PowerConsumption:
    """Power consumption of the whole PC: current power, totals and a chart of the logged values (energy.py,
    energy_view.py). Logging is switched on and configured on the Toolbox page."""


class FanControl:
    """Fan control: curves for the graphics card fans and the motherboard fan headers (fans.py, fan_view.py)."""


class DriveHealth:
    """Health of the drives like CrystalDiskInfo: status, temperature, life, SMART table (drive_view.py)."""


VIEWS = {PowerConsumption: EnergyView, FanControl: FanView, DriveHealth: DriveHealthView}   # tabs without a list


# ---------- Displaying values ----------
# Sensor types in the order in which the tab with AllSensors() lists them; shown as t("type.<sensor type>")
SENSOR_TYPES = (
    "Temperature", "Load", "Clock", "Frequency", "Power", "Voltage", "Current", "Energy", "Fan", "Control", "Flow",
    "Data", "SmallData", "Throughput", "Level", "Factor", "TimeSpan", "Timing", "Noise", "Conductivity", "Humidity",
)

UNITS = {   # sensor type -> (decimals, unit)
    "Temperature": (0, "°C"), "Load": (0, "%"), "Control": (0, "%"), "Level": (0, "%"),
    "Humidity": (0, "%"), "Clock": (0, "MHz"), "Frequency": (0, "Hz"), "Power": (1, "W"),
    "Voltage": (3, "V"), "Current": (2, "A"), "Fan": (0, "RPM"),
    "Flow": (0, "L/h"), "Data": (1, "GB"), "SmallData": (0, "MB"), "Timing": (1, "ns"),
    "Noise": (0, "dBA"), "Conductivity": (0, "µS/cm"), "Factor": (2, ""),
}
BAR_TYPES = {"Temperature", "Load", "Control", "Level", "Humidity"}      # values from 0–100 get a bar
NO_TRAFFIC_LIGHT = ("*Distance to TjMax", "Warning Temperature", "Critical Temperature")   # limits, not measurements


def fmt_rate(bytes_per_s):
    for unit in ("B/s", "KB/s", "MB/s", "GB/s"):
        if bytes_per_s < 1024 or unit == "GB/s":
            return f"{num(bytes_per_s, 0 if unit == 'B/s' else 1)} {unit}"
        bytes_per_s /= 1024


def fmt_value(sensor_type, value):
    if sensor_type == "Throughput":
        return fmt_rate(value)
    if sensor_type == "TimeSpan":
        minutes = int(value // 60)
        return f"{minutes // 60}:{minutes % 60:02d} h"
    if sensor_type == "Data" and value >= 1024:          # large amounts of data more readable in TB
        return f"{num(value / 1024, 2)} TB"
    if sensor_type == "Energy":                          # LHM delivers mWh, e.g. battery capacity
        return f"{num(value / 1000, 1)} Wh"
    if sensor_type == "SmallData" and value >= 1024:     # e.g. graphics memory rather in GB
        return f"{num(value / 1024, 1)} GB"
    if sensor_type == "Factor" and value == int(value):  # counters like Power On Hours without ".00"
        return num(value, 0)
    decimals, unit = UNITS.get(sensor_type, (1, ""))
    return f"{num(value, decimals)} {unit}".strip()


TRAFFIC_LIGHT = {"gray": (143, 163, 184), "green": (61, 220, 151), "yellow": (245, 196, 81), "red": (255, 100, 100)}


def temp_level(value, settings, kind):
    """"green", "yellow" or "red" depending on the limits from the settings ("gray" without a value).
    kind: "cpu" or "gpu" – both have their own limits."""
    if value is None:
        return "gray"
    if value < settings[f"{kind}_warn_temp"]:
        return "green"
    return "yellow" if value < settings[f"{kind}_hot_temp"] else "red"


def temp_color(value, settings, kind):
    """Traffic-light color (RGB) for a temperature – the same in tray icons and window."""
    return TRAFFIC_LIGHT[temp_level(value, settings, kind)]


def hex_color(rgb):
    return "#%02x%02x%02x" % rgb


def bar(value):
    """Small text bar for values from 0–100, e.g. ████░░░░░░ for 40 %."""
    filled = round(max(0.0, min(value, 100.0)) / 10)
    return "█" * filled + "░" * (10 - filled)


def explain(sensor, content):
    """Explanation from EXPLANATIONS in detail_window_content.py (first matching entry) – the texts themselves are
    in i18n.json."""
    for sensor_type, pattern, text in getattr(content, "EXPLANATIONS", []):
        if sensor_type == sensor["type"]:
            match = re.fullmatch(pattern, sensor["name"])
            if match:
                return label_text(text).format(*(g or "" for g in match.groups()))
    text = getattr(content, "TYPE_EXPLANATIONS", {}).get(sensor["type"])
    return label_text(text) if text else t("explain.default")


# ---------- From the content to the list ----------
def visible_rows(tree, parent=""):
    """All rows of a Treeview in display order – without those in collapsed areas."""
    for key in tree.get_children(parent):
        yield key
        if tree.item(key, "open"):
            yield from visible_rows(tree, key)


def ancestors(tree, key):
    """The parent rows of key (for the indentation when copying)."""
    while key := tree.parent(key):
        yield key


def needed_devices(tab):
    """Which devices the measuring thread has to read for this tab (None = all). "power" = the power of CPU and
    graphics card for the tab Power (measured even while logging is off)."""
    if is_hardware_data(tab):
        return set()                                     # measure nothing, only the icons keep running
    if is_power(tab):
        return {"power"}
    if is_fans(tab):
        return {"gpu", "motherboard"}                    # speeds and control values of the fans
    if view_class(tab) is DriveHealthView:
        return {"health"}                                # SMART of all drives (read_drive_health)
    if any(isinstance(g, AllSensors) for g in tab.groups):
        return None
    return {g.device for g in tab.groups}


def is_hardware_data(tab):
    return any(isinstance(g, HardwareData) for g in tab.groups)


def is_power(tab):
    return any(isinstance(g, PowerConsumption) for g in tab.groups)


def is_fans(tab):
    return any(isinstance(g, FanControl) for g in tab.groups)


def view_class(tab):
    """The own view of a tab without a sensor list (EnergyView, FanView, DriveHealthView) – or None."""
    return next((VIEWS[type(g)] for g in tab.groups if type(g) in VIEWS), None)


def with_percent(tab):
    """True if the tab has rows with two values (Section with one_row) – bars then get "97 %"."""
    return any(isinstance(v, Section) and v.one_row for g in tab.groups for v in getattr(g, "values", ()))


def hardware_data_rows(data, overview=False, private=False):
    """Like rows(), only for the tab Hardware data: category -> device -> fields. Fields are in it as an "Info"
    sensor (with "field" = ID of the field for the explanation). overview=True: only category -> model names,
    identical devices combined ("2× …"). private: privacy mode – sensitive values as ████████ ("hidden": True)."""
    result = []
    for c, (category, devices) in enumerate(data):
        result.append((f"c{c}", "", hardware_data.category_name(category), None))
        if overview:
            result += [(f"c{c}|{d}", f"c{c}", name, None)
                       for d, name in enumerate(hardware_data.model_names(devices))]
            continue
        for d, (name, fields) in enumerate(devices):
            head = f"c{c}|{d}"
            result.append((head, f"c{c}", name, None))
            for i, (field, value) in enumerate(fields):
                text = hardware_data.shown(field, value, private)
                result.append((f"{head}|{i}", head, hardware_data.field_name(field),
                               {"type": "Info", "name": hardware_data.field_name(field), "field": field,
                                "value": text, "hidden": text != value}))
    return result


def rows(tab, snap):
    """What the tab currently shows, as a list of (key, parent key, text, sensor or None).
    Headings have no sensor. The keys stay the same as long as nothing changes in the structure."""
    result = []
    for nr, group in enumerate(tab.groups):
        for hw in snap["hardware"]:
            head = f"{nr}|{hw['id']}"
            if isinstance(group, AllSensors):
                result.append((head, "", hw["name"], None))
                for sensor_type in SENSOR_TYPES:                # sorted by sensor type
                    sensors = [s for s in hw["sensors"] if s["type"] == sensor_type]
                    if sensors:
                        result.append((f"{head}|{sensor_type}", head, t(f"type.{sensor_type}"), None))
                        result += [(f"{head}|{sensor_type}|{i}", f"{head}|{sensor_type}", s["name"], s)
                                   for i, s in enumerate(sensors)]
                continue
            if hw["device"] != group.device or not fnmatch.fnmatchcase(hw["name"].lower(), group.name.lower()):
                continue
            group_rows = []
            for i, value in enumerate(group.values):
                if isinstance(value, Section):                  # sub-item with its own rows below it
                    sub = value.rows(hw["sensors"])
                    if sub:
                        key = f"{head}|{value.title}"           # ID: collapsed stays collapsed, also after a language switch
                        group_rows.append((key, head, label_text(value.title), None))
                        group_rows += [(f"{key}|{j}", key, text, s) for j, (text, s) in enumerate(sub)]
                else:
                    group_rows += [(f"{head}|{i}|{j}", head, label_text(value.text) if value.text else s["name"], s)
                                   for j, s in enumerate(s for s in hw["sensors"] if value.matches(s))]
            if group_rows:
                result.append((head, "", label_text(group.title) if group.title else hw["name"], None))
                result += group_rows
    return result


# ---------- Windows helpers ----------
def work_area(x, y):
    """Screen area without the taskbar (left, top, right, bottom) of the screen at point x, y – or None."""
    class MONITORINFO(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_ulong), ("rcMonitor", ctypes.c_long * 4),
                    ("rcWork", ctypes.c_long * 4), ("dwFlags", ctypes.c_ulong)]

    user32 = ctypes.windll.user32
    user32.MonitorFromPoint.restype = ctypes.c_void_p
    user32.MonitorFromPoint.argtypes = [wintypes.POINT, ctypes.c_ulong]
    monitor = user32.MonitorFromPoint(wintypes.POINT(x, y), 0)         # 0 = None if there is no screen there
    if not monitor:
        return None
    info = MONITORINFO(cbSize=ctypes.sizeof(MONITORINFO))
    user32.GetMonitorInfoW.argtypes = [ctypes.c_void_p, ctypes.POINTER(MONITORINFO)]
    return tuple(info.rcWork) if user32.GetMonitorInfoW(monitor, ctypes.byref(info)) else None


def dark_title_bar(window):
    """Windows title bar in the dark design (matching the window content)."""
    hwnd = ctypes.windll.user32.GetParent(window.winfo_id())
    on = ctypes.c_int(1)
    ctypes.windll.dwmapi.DwmSetWindowAttribute(hwnd, 20, ctypes.byref(on), ctypes.sizeof(on))  # IMMERSIVE_DARK_MODE


# ---------- The window ----------
# the same colors as the dark Toolbox (toolbox/base.py) – the window uses ttk instead of CustomTkinter
BG, BG_LIST, FG, SUB, HOVER, ACCENT = "#0a1d2c", "#152736", "#e2e8f0", "#8fa3b8", "#213443", "#3ddc97"
SELECTED = "#1d4f47"             # selected rows (like selected entries in the Toolbox)
FONT, FONT_GROUP = ("Segoe UI", 9), ("Segoe UI Semibold", 10)       # rows and device names
SIZE = (560, 640)                # initial size at 100 % scaling
TITLE_BAR = 32                   # height of the Windows title bar (doesn't count for the window size)
HINT = "window.hint"             # ID of the hint in the footer (clicking, selecting, copying)


class DetailWindow:
    """A normal window: movable and resizable, ✕ only hides it. Position and size are saved.

    app (HardwareMonitorApp) provides: settings, kinds (the icons shown), current, present, snapshot() and
    window_needs(devices) – tells the measuring thread which devices are needed right now (False = window closed).
    """

    def __init__(self, root, app):
        self.app = app
        self.scale = root.winfo_fpixels("1i") / 96      # 1.0 at 100 %, 1.5 at 150 %
        self.content = None                              # the module detail_window_content
        self.content_mtime = None                        # modification time of the file at the last load
        self.error = None                                # error message if the file couldn't be loaded
        self.tabs = []                                   # [(Tab, Treeview)]
        self.hidden_for = None                           # present devices by which tabs were last hidden
        self.signature = {}                              # per Treeview: structure at the last drawing
        self.collapsed = {}                              # per Treeview: keys of the collapsed areas
        self.sensors = {}                                # row key -> sensor (for the explanation)
        self.copyable = {}                               # row key -> (column, text) that a click copies
        self.press = None                                # mouse button pressed in the list: click or drag?
        self.fonts = {}                                  # for measuring the texts (over_value)
        self.buttons = {}                                # per Treeview: category row -> "Copy" button above it
        self.message_job = None                          # "… copied" is in the footer right now
        self.hw_data = None                              # hardware data read last (hardware_data.read_all())
        self.hw_data_current = False                     # False = read again the next time it's shown
        self.hw_data_thread = None
        self.hw_data_new = None                          # result of the reading thread until hw_data_wait() takes it

        win = self.win = tk.Toplevel(root)
        self.hw_view = tk.StringVar(win, "overview")     # sub-tab in the tab Hardware data: overview / details
        win.withdraw()
        win.title(str(PROGRAM_FILE))                     # path like Explorer – see PROGRAM_FILE
        win.configure(bg=BG)
        win.minsize(self.px(320), self.px(240))
        try:
            win.iconbitmap(str(ICON_FILE))
        except tk.TclError:
            pass
        self.style(win)

        # Header: the two icon values in large
        header = ttk.Frame(win)
        header.pack(fill="x", padx=self.px(14), pady=(self.px(10), self.px(4)))
        self.header = {}
        for kind in app.kinds:                           # like the taskbar icons – without GPU temperature only CPU
            ttk.Label(header, text=kind.upper(), style="HeaderName.TLabel").pack(side="left", padx=(0, self.px(6)))
            self.header[kind] = ttk.Label(header, text="–", style="HeaderValue.TLabel")
            self.header[kind].pack(side="left", padx=(0, self.px(24)))

        # far right the gear: settings of the monitor – they are on its page in the Toolbox
        gear = ttk.Label(header, text="⚙", style="Gear.TLabel", cursor="hand2")
        gear.pack(side="right")
        gear.bind("<Button-1>", lambda e: self.open_settings())
        gear.bind("<Enter>", lambda e: (gear.configure(foreground=ACCENT), self.footer(t("window.gear"))))
        gear.bind("<Leave>", lambda e: (gear.configure(foreground=SUB), self.footer(t(HINT))))

        # left of it: "Loading data …" until the first matching measurement is there, then "● Live"
        self.status = ttk.Label(header, style="HeaderName.TLabel")
        self.status.pack(side="right", padx=(0, self.px(12)))
        self.live = None                                 # True = "● Live" is already shown (stays until closing)
        self.set_status(False)

        # left of the status: privacy mode on/off (the same as in the right-click menu of the icons)
        self.privacy = ttk.Label(header, style="HeaderName.TLabel", cursor="hand2")
        self.privacy.pack(side="right", padx=(0, self.px(16)))
        self.privacy.bind("<Button-1>", lambda e: self.app.toggle_privacy())
        self.privacy.bind("<Enter>", lambda e: (self.privacy.configure(foreground=FG), self.footer(t(
            "window.privacy.on_help" if self.private() else "window.privacy.off_help"))))
        self.privacy.bind("<Leave>", lambda e: (self.show_privacy(), self.footer(t(HINT))))
        self.show_privacy()

        # Tab bar: own buttons that wrap into a second row when the window is too narrow (ttk.Notebook would squeeze
        # the titles until they're unreadable). The notebook below only switches the pages – its own tabs are hidden.
        self.tab_bar = ttk.Frame(win, height=self.px(26))
        self.tab_bar.pack(fill="x", padx=self.px(8))
        self.tab_bar.bind("<Configure>", lambda e: self.flow_tabs())
        self.tab_var = tk.IntVar(win, 0)
        self.tab_buttons = []                            # one per entry in self.tabs (same index)
        self.hidden_tabs = set()                         # indexes of tabs hidden by hide_missing_tabs()
        self.notebook = ttk.Notebook(win, style="Pages.TNotebook")
        self.notebook.pack(fill="both", expand=True, padx=self.px(8))
        self.notebook.bind("<<NotebookTabChanged>>", lambda e: (
            self.tab_var.set(self.notebook.index("current")), self.report(), self.update(),
            self.footer(self.tab_hint())))

        self.foot = ttk.Label(win, text=t(HINT), style="Foot.TLabel", justify="left")
        self.foot.pack(fill="x", padx=self.px(14), pady=self.px(8))
        self.foot.bind("<Configure>", lambda e: self.foot.configure(wraplength=e.width))

        win.protocol("WM_DELETE_WINDOW", self.hide)                  # ✕ only hides
        for key in ("<Control-c>", "<Control-C>"):
            win.bind(key, lambda e: self.copy_selection())
        for key in ("<Control-a>", "<Control-A>"):
            win.bind(key, lambda e: self.select_all())
        win.bind("<Escape>", lambda e: self.active_tree() and self.active_tree().selection_set(()))
        for event in ("<Map>", "<Unmap>"):                           # minimized = measure nothing
            win.bind(event, lambda e: e.widget is win and self.visibility_changed())
        win.update_idletasks()
        dark_title_bar(win)

    def px(self, value):
        return int(value * self.scale)

    def style(self, win):
        style = ttk.Style(win)
        style.theme_use("clam")                          # the only built-in theme that can be colored dark
        style.configure(".", background=BG, foreground=FG, bordercolor=BG, lightcolor=BG, darkcolor=BG,
                        troughcolor=BG_LIST, focuscolor=BG, font=FONT)
        style.configure("HeaderName.TLabel", foreground=SUB, font=("Segoe UI", 11))
        style.configure("HeaderValue.TLabel", font=("Segoe UI Semibold", 18))
        style.configure("Gear.TLabel", foreground=SUB, font=("Segoe UI Symbol", 16))
        style.configure("Foot.TLabel", foreground=SUB)
        style.configure("Pages.TNotebook", borderwidth=0, tabmargins=(0, 0, 0, 0))
        style.layout("Pages.TNotebook.Tab", [])          # no tabs of its own – the tab bar above is made of buttons
        style.configure("Tab.Toolbutton", background=BG, foreground=SUB, bordercolor=BG, lightcolor=BG,
                        darkcolor=BG, borderwidth=0, padding=(self.px(8), self.px(4)))
        style.map("Tab.Toolbutton", background=[("selected", BG_LIST), ("active", HOVER)],
                  foreground=[("selected", ACCENT), ("active", FG)], lightcolor=[("selected", BG_LIST)],
                  darkcolor=[("selected", BG_LIST)], bordercolor=[("selected", BG_LIST)])
        style.configure("Treeview", background=BG_LIST, fieldbackground=BG_LIST, foreground=FG, borderwidth=0,
                        rowheight=self.px(22))
        style.map("Treeview", background=[("selected", SELECTED)], foreground=[("selected", FG)],
                  bordercolor=[("focus", BG)])           # no bright border when the list has the focus
        style.configure("Small.TButton", background=HOVER, foreground=ACCENT, bordercolor=HOVER, lightcolor=HOVER,
                        darkcolor=HOVER, focuscolor=HOVER, borderwidth=0, padding=(self.px(8), self.px(2)), width=0)
        style.map("Small.TButton", background=[("pressed", FG), ("active", ACCENT)],
                  foreground=[("active", BG)], lightcolor=[("active", ACCENT)], darkcolor=[("active", ACCENT)],
                  bordercolor=[("active", ACCENT)])
        # Sub-tabs in the tab Hardware data: radio buttons without a dot, colored like the tabs above
        style.configure("SubTab.Toolbutton", background=BG, foreground=SUB, bordercolor=BG, lightcolor=BG,
                        darkcolor=BG, borderwidth=0, padding=(self.px(8), self.px(2)))
        style.map("SubTab.Toolbutton", background=[("selected", BG_LIST), ("active", HOVER)],
                  foreground=[("selected", ACCENT), ("active", FG)], lightcolor=[("selected", BG_LIST)],
                  darkcolor=[("selected", BG_LIST)], bordercolor=[("selected", BG_LIST)])
        style.configure("Vertical.TScrollbar", background=HOVER, troughcolor=BG_LIST, bordercolor=BG_LIST,
                        arrowcolor=SUB, lightcolor=HOVER, darkcolor=HOVER, gripcount=0)
        # Tab Power: tiles with a label, a large value and a line below it
        style.configure("Tile.TFrame", background=BG_LIST)
        style.configure("TileLabel.TLabel", background=BG_LIST, foreground=SUB, font=("Segoe UI", 8))
        style.configure("TileValue.TLabel", background=BG_LIST, foreground=FG, font=("Segoe UI Semibold", 13))

    # --- Loading the content ---
    def load_content(self):
        """Reads detail_window_content.py (again) if it has changed since the last time."""
        try:
            mtime = CONTENT_FILE.stat().st_mtime
        except OSError:
            mtime = None
        if self.content is not None and mtime == self.content_mtime:
            return
        try:
            if self.content is None:
                import detail_window_content as module
            else:
                module = importlib.reload(self.content)
            tabs = list(module.TABS)
        except Exception as error:                       # typo in the file -> the window works anyway
            logging.exception("detail_window_content.py could not be loaded")
            self.content_mtime = mtime
            if not self.tabs:
                self.build_tabs([Tab("tab.all", AllSensors())])
            self.error = f"⚠  {t('window.content_error', error=error)}"
            self.foot.configure(text=self.error, foreground=hex_color(TRAFFIC_LIGHT["red"]))
            return
        self.content, self.content_mtime, self.error = module, mtime, None
        self.foot.configure(text=t(HINT), foreground=SUB)
        self.build_tabs(tabs)

    def build_tabs(self, tabs):
        selected = self.active_tab()[0].title if self.tabs else None
        for tab_frame in self.notebook.tabs():
            self.notebook.forget(tab_frame)
        for _, tree in self.tabs:
            tree.master.destroy()
        self.tabs, self.signature, self.collapsed = [], {}, {}
        self.hidden_for = None                           # new tabs are all visible

        self.buttons = {}
        for tab in tabs:
            frame = ttk.Frame(self.notebook)
            if view_class(tab):                          # no sensor list: own view (energy_, fan_, drive_view.py)
                frame.configure(padding=(self.px(6), 0))
                self.notebook.add(frame, text=label_text(tab.title))
                self.tabs.append((tab, view_class(tab)(frame, self)))
                continue
            if is_hardware_data(tab):                    # on top: sub-tabs Overview / Details and "Copy all"
                bar_frame = ttk.Frame(frame)
                bar_frame.pack(side="top", fill="x", pady=(self.px(6), self.px(4)))
                for view in ("overview", "details"):
                    sub_tab = ttk.Radiobutton(bar_frame, text=t(f"hw.{view}"), value=view, variable=self.hw_view,
                                              style="SubTab.Toolbutton", cursor="hand2", command=self.update)
                    sub_tab.pack(side="left")
                    sub_tab.bind("<Enter>", lambda e, v=view: self.footer(t(f"hw.{v}.help")))
                    sub_tab.bind("<Leave>", lambda e: self.footer(t(HINT)))
                button = ttk.Button(bar_frame, text=f"⧉  {t('hw.copy_all')}", style="Small.TButton",
                                    cursor="hand2", command=self.copy_all)
                button.pack(side="right")
                button.bind("<Enter>", lambda e: self.footer(
                    t("hw.copy_all.overview" if self.overview() else "hw.copy_all.details")))
                button.bind("<Leave>", lambda e: self.footer(t(HINT)))
            tree = ttk.Treeview(frame, columns=("value", "bar"), show="tree", selectmode="extended")
            if is_hardware_data(tab):                    # field name on the left, long value (IDs) next to it
                tree.column("#0", width=self.px(300), stretch=True)     # wide enough for long device names
                tree.column("value", width=self.px(260), stretch=True, anchor="w")
                tree.column("bar", width=0, minwidth=0, stretch=False)
            else:
                tree.column("#0", width=self.px(250), stretch=True)
                tree.column("value", width=self.px(100), stretch=False, anchor="e")
                if with_percent(tab):                    # bar with "97 %" behind it: wider, all bars left-aligned
                    tree.column("bar", width=self.px(140), stretch=False, anchor="w")
                else:
                    tree.column("bar", width=self.px(100), stretch=False, anchor="center")
            tree.tag_configure("category", foreground=SUB, font=("Segoe UI Semibold", 9))
            tree.tag_configure("group", font=FONT_GROUP)
            tree.tag_configure("type", foreground=SUB, font=("Segoe UI", 8))
            tree.tag_configure("hint", foreground=SUB)
            for level, rgb in TRAFFIC_LIGHT.items():     # temperature rows in traffic-light color
                tree.tag_configure(level, foreground=hex_color(rgb))
            scroll = ttk.Scrollbar(frame, orient="vertical", command=tree.yview)
            # the "Copy" buttons lie on top of the list and move along when scrolling, expanding and resizing
            tree.configure(yscrollcommand=lambda *a, s=scroll, tr=tree: (s.set(*a), self.place_buttons(tr)))
            for event in ("<Configure>", "<<TreeviewOpen>>", "<<TreeviewClose>>"):
                tree.bind(event, lambda e, tr=tree: self.win.after_idle(self.place_buttons, tr))
            scroll.pack(side="right", fill="y")
            tree.pack(side="left", fill="both", expand=True)
            tree.bind("<Motion>", lambda e, tr=tree: self.describe(tr, e))
            tree.bind("<Leave>", lambda e, tr=tree: self.describe(tr, None))
            # selecting by click, Ctrl+click and Shift+click is done by ttk itself – dragging and copying come on top
            tree.bind("<ButtonPress-1>", lambda e, tr=tree: self.on_press(tr, e))
            tree.bind("<B1-Motion>", lambda e, tr=tree: self.on_drag(tr, e))
            tree.bind("<ButtonRelease-1>", lambda e, tr=tree: self.on_release(tr, e))
            tree.bind("<Button-3>", lambda e, tr=tree: self.context_menu(tr, e))
            self.notebook.add(frame, text=label_text(tab.title))
            self.tabs.append((tab, tree))

        for button in self.tab_buttons:
            button.destroy()
        self.tab_buttons, self.hidden_tabs = [], set()
        for index, (tab, _) in enumerate(self.tabs):
            self.tab_buttons.append(ttk.Radiobutton(
                self.tab_bar, text=label_text(tab.title), value=index, variable=self.tab_var, style="Tab.Toolbutton",
                cursor="hand2", command=lambda i=index: self.notebook.select(i)))
        self.flow_tabs()

        titles = [tab.title for tab, _ in self.tabs]
        if selected in titles:
            self.notebook.select(titles.index(selected))
        self.tab_var.set(self.notebook.index("current") if self.tabs else 0)

    def flow_tabs(self):
        """Tab buttons side by side; whatever doesn't fit into the row anymore wraps into the next one."""
        width = self.tab_bar.winfo_width()
        if width <= 1:                                   # not laid out yet
            width = self.px(SIZE[0] - 16)
        x = y = row_height = 0
        for index, button in enumerate(self.tab_buttons):
            if index in self.hidden_tabs:
                button.place_forget()
                continue
            w, h = button.winfo_reqwidth(), button.winfo_reqheight()
            if x and x + w > width:
                x, y, row_height = 0, y + row_height, 0
            button.place(x=x, y=y)
            x, row_height = x + w, max(row_height, h)
        height = y + row_height
        if height and height != self.tab_bar.winfo_reqheight():
            self.tab_bar.configure(height=height)

    def active_tab(self):
        return self.tabs[self.notebook.index("current")]     # counts hidden tabs too, so it matches self.tabs

    def tab_hint(self):
        """Footer text when nothing else is shown: depends on the tab (list, Power, Fans)."""
        view = self.active_tab()[1] if self.tabs else None
        hints = {EnergyView: "power.hint", FanView: "fans.editor.help", DriveHealthView: "health.hint"}
        return t(hints.get(type(view), HINT))

    def active_tree(self):
        """The list of the selected tab – or None (no tabs yet, or the tab Power, which has a chart instead)."""
        if not self.tabs:
            return None
        tree = self.active_tab()[1]
        return tree if isinstance(tree, ttk.Treeview) else None

    def hide_missing_tabs(self):
        """Hide tabs for devices this PC doesn't have (e.g. motherboard sensors on a laptop).
        Only possible once all hardware is loaded after the start – until then all tabs are visible."""
        present = self.app.present
        if present is None or present == self.hidden_for:
            return
        self.hidden_for = present                        # before hide(): that triggers a tab change and thus us again
        for index, (tab, tree) in enumerate(self.tabs):
            devices = needed_devices(tab)                # empty for Hardware data, None for AllSensors -> stay
            if devices and not devices & (present | {"power", "health"}):   # not real devices – always there
                self.hidden_tabs.add(index)
                self.notebook.hide(tree.master)
        self.flow_tabs()

    # --- Show / hide ---
    def visible(self):
        return self.win.state() in ("normal", "zoomed")      # zoomed = maximized

    def toggle(self):
        self.hide() if self.visible() else self.show()

    def show(self, tab=None):
        """Show the window – with tab (title ID, e.g. "tab.power") on that tab."""
        self.load_content()
        titles = [shown_tab.title for shown_tab, _ in self.tabs]
        if tab in titles:
            self.notebook.select(titles.index(tab))
        if self.win.state() == "withdrawn":
            self.place()
        self.win.deiconify()
        self.win.lift()
        self.win.focus_force()
        self.update()

    def hide(self):
        self.save_geometry()
        self.win.withdraw()

    def place(self):
        """Saved position – or the first time at the bottom right of the screen where the click happened."""
        saved = self.app.settings.get("window", "")
        match = re.fullmatch(r"(\d+)x(\d+)\+(-?\d+)\+(-?\d+)", saved)
        if match:
            w, h, x, y = map(int, match.groups())
            if work_area(x + w // 2, y + h // 2):        # still on a connected screen?
                self.win.geometry(saved)
                return
        mx, my = self.win.winfo_pointerxy()
        left, top, right, bottom = (work_area(mx, my)
                                    or (0, 0, self.win.winfo_screenwidth(), self.win.winfo_screenheight()))
        w, h = self.px(SIZE[0]), min(self.px(SIZE[1]), bottom - top - self.px(24 + TITLE_BAR))
        self.win.geometry(f"{w}x{h}+{right - w - self.px(12)}+{bottom - h - self.px(12 + TITLE_BAR)}")

    def save_geometry(self):
        """Remember position and size (in data\\hardware_monitor.json)."""
        if self.visible():
            self.app.window_geometry(self.win.geometry())

    def visibility_changed(self):
        """Window opened or restored -> measure right away; hidden or minimized -> measure nothing."""
        if not self.visible():
            self.set_status(False, reset=True)           # the next time it opens, "Loading data …" first again
            self.hw_data_current = False                 # read again the next time it opens (e.g. new USB devices)
        self.report()

    def set_status(self, loaded, reset=False):
        """Top right: "● Live" as soon as the first values are there after opening, before that "Loading data …".
        Once "Live", it stays until closing – even if a tab change briefly waits for new values.
        reset=True resets it (window closed or minimized)."""
        live = bool(loaded or (self.live and not reset))
        if live != self.live:
            self.live = live
            self.status.configure(text=f"●  {t('window.live')}" if live else t("window.loading"),
                                  foreground=ACCENT if live else SUB)

    def private(self):
        """True while privacy mode is on."""
        return bool(self.app.settings.get("privacy_mode"))

    def show_privacy(self):
        """Switch in the header and window title according to privacy mode. The title normally shows the path of
        the program – it can contain the Windows user name (C:\\Users\\<name>\\…), so in privacy mode only the name."""
        on = self.private()
        self.privacy.configure(text=f"{'☑' if on else '☐'}  {t('window.privacy')}", foreground=ACCENT if on else SUB)
        title = t("program.name") if on else str(PROGRAM_FILE)
        if self.win.title() != title:
            self.win.title(title)

    def privacy_changed(self):
        """Privacy mode was switched (here, in the right-click menu or in the Toolbox): show everything anew."""
        self.show_privacy()
        if self.message_job:
            self.win.after_cancel(self.message_job)
            self.message_end()
        self.update()

    def language_changed(self):
        """The language was switched in the Toolbox: tabs and all fixed texts are renewed right away – the hardware
        data is read again in the new language. If the window is closed, that happens the next time it opens."""
        self.hw_data_current = False
        self.content_mtime = None                        # load_content() then rebuilds the tabs
        live, self.live = self.live, None
        self.set_status(live)
        self.show_privacy()
        if self.message_job:
            self.win.after_cancel(self.message_job)
        self.message_end()                               # footer in the new language
        if self.visible():
            self.load_content()
            self.update()

    def open_settings(self):
        """Gear: open the Toolbox on the Hardware Monitor's page (in the background, open_toolbox)."""
        result = []
        threading.Thread(target=lambda: result.append(open_toolbox()), daemon=True).start()

        def wait():
            if not result:
                self.win.after(250, wait)
            elif not result[0]:
                self.confirm(t("window.toolbox_error"), color=hex_color(TRAFFIC_LIGHT["red"]))

        self.win.after(250, wait)

    def report(self):
        """Tell the measuring thread what's needed: the devices of the selected tab – or nothing (window closed)."""
        if self.visible() and self.tabs:
            self.app.window_needs(needed_devices(self.active_tab()[0]))
        else:
            self.app.window_needs(False)

    # --- Showing values ---
    def update(self):
        """Fill in new measurements. If the structure changes (other tab, new sensors), the list is refilled."""
        if not self.visible() or not self.tabs:
            return
        self.hide_missing_tabs()
        self.show_privacy()                              # e.g. switched in the Toolbox in the meantime
        snap = self.app.snapshot()
        for kind, label in self.header.items():
            value = self.app.current.get(kind)
            label.configure(text="–" if value is None else f"{value:.0f} °C",
                            foreground=hex_color(temp_color(value, self.app.settings, kind)))

        tab, tree = self.active_tab()
        if isinstance(tree, EnergyView):                 # tab Power: tiles and chart instead of a list
            self.set_status(self.app.power_now is not None)
            tree.update()
            return
        if isinstance(tree, FanView):                    # tab Fans: list of fans and the curve editor
            measured = snap.get("devices")
            self.set_status(self.app.hardware_ready and (measured is None or {"gpu", "motherboard"} <= measured))
            tree.update()
            return
        if isinstance(tree, DriveHealthView):            # tab Drive health: drives and SMART table
            self.set_status(self.app.drive_health is not None)
            tree.update()
            return
        info = is_hardware_data(tab)
        if info:
            self.load_hw_data()
            tab_rows = hardware_data_rows(self.hw_data or [], self.overview(), self.private())
            empty = t("hw.no_data") if self.hw_data == [] else t("hw.reading")
            loaded = self.hw_data_current
        else:
            needed, measured = needed_devices(tab), snap.get("devices")
            if measured is not None and (needed is None or not needed <= measured):
                tab_rows = []                            # a matching measurement is coming right away
                empty = t("window.loading_sensors")
                loaded = False
            else:
                tab_rows = rows(tab, snap)
                empty = t("window.no_sensors")
                loaded = True
            if snap.get("loading"):                      # right after the program start hardware is still loading
                empty = t("window.loading_sensors")
                loaded = False
        self.set_status(loaded)

        signature = [(key, parent, text) for key, parent, text, _ in tab_rows] or [empty]
        if signature != self.signature.get(tree):
            collapsed = self.collapsed.setdefault(tree, set())   # collapsed areas stay collapsed (e.g. after a tab change)
            for key in visible_rows(tree):
                if tree.get_children(key):
                    (collapsed.add if not tree.item(key, "open") else collapsed.discard)(key)
            tree.delete(*tree.get_children())
            for key, parent, text, sensor in tab_rows:
                if sensor:
                    tag = ()
                elif not parent:                         # top level: device – for Hardware data the category
                    tag = ("category",) if info else ("group",)
                else:                                    # second level: sensor type – for Hardware data the device
                    tag = ("group",) if info else ("type",)
                tree.insert(parent, "end", iid=key, text=text, open=key not in collapsed, tags=tag)
            if not tab_rows:
                tree.insert("", "end", iid="empty", text=empty, tags=("hint",))
            self.signature[tree] = signature
            if info:
                self.hw_columns(tree)
                self.build_buttons(tree, [(f"c{c}", category) for c, (category, _) in enumerate(self.hw_data or [])])

        self.sensors, self.copyable = {}, {}
        device_of = {s["id"]: hw["device"] for hw in snap["hardware"] for s in hw["sensors"]}
        for key, _, text, sensor in tab_rows:
            if sensor is None:
                if "group" in tree.item(key, "tags"):    # device name -> clicking it copies it
                    self.copyable[key] = ("#0", text)
                continue
            self.sensors[key] = sensor
            if sensor["type"] == "Info":                 # Hardware data: fixed value, clicking it copies it
                self.copyable[key] = ("#1", sensor["value"])
                tree.item(key, values=(sensor["value"], ""))
                continue
            is_temp = (sensor["type"] == "Temperature"
                       and not any(fnmatch.fnmatchcase(sensor["name"], m) for m in NO_TRAFFIC_LIGHT))
            value = fmt_value(sensor["type"], sensor["value"])
            bar_text = bar(sensor["value"]) if sensor["type"] in BAR_TYPES else ""
            # graphics card by the GPU limits, everything else (CPU, motherboard, drives …) by the CPU limits
            kind = "gpu" if device_of.get(sensor["id"]) == "gpu" else "cpu"
            tags = (temp_level(sensor["value"], self.app.settings, kind),) if is_temp else ()
            second = sensor.get("bar")                   # second value in the same row, e.g. fan control
            if second:
                bar_text = f"{bar(second['value'])}  {fmt_value(second['type'], second['value'])}"
                if sensor["type"] == "Fan" and sensor["value"] == 0:
                    value, tags = t("value.stopped"), ("gray",)    # driven, but not spinning (or nothing attached)
            self.copyable[key] = ("#1", value)           # measurement as it is shown right now
            tree.item(key, values=(value, bar_text), tags=tags)

    def describe(self, tree, event):
        """Mouse over the list (event None = left): hand cursor exactly over copyable values, the explanation of the
        row in the footer."""
        key = tree.identify_row(event.y) if event else None
        tree.configure(cursor="hand2" if key and self.over_value(tree, key, event.x) else "")
        if self.error or self.message_job:
            return                                       # leave the error message or "… copied" in place
        sensor = self.sensors.get(key)
        if sensor and sensor["type"] == "Info":
            explanation = hardware_data.explanation(sensor["field"])
            text = (f"{sensor['name']}: {sensor['value']}\n"     # full value in case the column is too narrow
                    f"{explanation + ' ' if explanation else ''}"
                    f"{t('window.privacy.hidden') if sensor.get('hidden') else t('window.click_value')}")
        elif sensor:
            text = f"{sensor['name']}: {explain(sensor, self.content)}"
            if sensor.get("bar"):
                text += " " + t("window.the_bar", text=explain(sensor["bar"], self.content))
            if sensor.get("bar") and sensor["type"] == "Fan" and sensor["value"] == 0:
                text += " " + t("window.stopped_explained")
        elif key in self.copyable:
            text = t("window.click_name", name=self.copyable[key][1])
        else:
            text = t(HINT)
        self.foot.configure(text=text, foreground=SUB)

    def footer(self, text):
        """Help text of the buttons (mouse over) – error message and "… copied" take precedence."""
        if not (self.error or self.message_job):
            self.foot.configure(text=text, foreground=SUB)

    # --- Selecting and copying ---
    # A click selects the row (ttk does that, as well as Ctrl+click and Shift+click). If it is exactly on a value
    # or device name, that is copied as well – not on the label in front of it or in the empty space behind it.
    # Dragging over several rows with the mouse button pressed selects all in between; Ctrl+C copies the selected.
    def on_press(self, tree, event):
        """Mouse button pressed: only remember where – only on release is it clear whether it was a click or a drag."""
        key = tree.identify_row(event.y)
        tree.focus_set()
        if not key:
            tree.selection_set(())                       # a click into empty space clears the selection
        self.press = {"key": key, "x": event.x, "dragged": False,
                      "modifier": bool(event.state & 0x0005),        # Shift or Ctrl: only select
                      "arrow": tree.identify_element(event.x, event.y).endswith("indicator")}  # only expand/collapse

    def on_drag(self, tree, event):
        """With the mouse button pressed over several rows: select all in between. At the edge it scrolls along."""
        p = self.press
        if not p or not p["key"] or p["modifier"] or p["arrow"]:
            return
        if event.y < 0:
            tree.yview_scroll(-1, "units")
            key = tree.identify_row(1)
        elif event.y >= tree.winfo_height():
            tree.yview_scroll(1, "units")
            key = tree.identify_row(tree.winfo_height() - 2)
        else:
            key = tree.identify_row(event.y)
        if not key or (key == p["key"] and not p["dragged"]):
            return
        keys = list(visible_rows(tree))
        if p["key"] not in keys or key not in keys:      # the list has been rebuilt in the meantime
            return
        p["dragged"] = True
        a, b = sorted((keys.index(p["key"]), keys.index(key)))
        tree.selection_set(keys[a:b + 1])

    def on_release(self, tree, event):
        """A simple click (not dragged, without Ctrl/Shift) exactly on a value copies it."""
        p, self.press = self.press, None
        if not p or p["dragged"] or p["modifier"] or p["arrow"] or tree.identify_row(event.y) != p["key"]:
            return
        text = self.over_value(tree, p["key"], p["x"])
        if text:
            self.to_clipboard(text, t("window.copied", text=text))

    def over_value(self, tree, key, x):
        """The text a click at position x in row key copies – only directly on the value or device name, not on the
        label next to it (e.g. "Product ID") or in the empty space behind it. Otherwise None."""
        column, text = self.copyable.get(key, (None, None))
        box = tree.bbox(key, column) if text and tree.identify_column(x) == column else ""
        if not box:
            return None
        left, top, width, height = box
        font = FONT_GROUP if "group" in tree.item(key, "tags") else FONT
        if font not in self.fonts:
            self.fonts[font] = tkfont.Font(self.win, font=font)
        length = self.fonts[font].measure(text)
        if column == "#0":                               # name: starts behind the indentation and the arrow
            start = left
            while start < left + width and tree.identify_element(start, top + height // 2) != "text":
                start += 1
        elif str(tree.column(column, "anchor")) == "e":  # measurements are right-aligned (str: Tk returns a Tcl object)
            start = left + width - self.px(5) - length
        else:
            start = left + self.px(5)                    # ttk leaves about 5 px free on the left
        return text if start - self.px(4) <= x <= start + length + self.px(4) else None

    def row_as_text(self, tree, key):
        """One row for copying: "Field: value" – headings (device, category, sensor type) only with their name."""
        column, value = self.copyable.get(key, (None, None))
        name = tree.item(key, "text")
        second = self.sensors.get(key, {}).get("bar")
        if second:                                       # e.g. "Fan #2: 2523 RPM, 97 %"
            value = f"{value}, {fmt_value(second['type'], second['value'])}"
        return f"{name}: {value}" if column == "#1" else name

    def copy_selection(self):
        """Ctrl+C: the selected rows of the tab as text, indented like in the list."""
        tree = self.active_tree()
        if not tree:
            return
        selected = set(tree.selection())
        keys = [key for key in visible_rows(tree) if key in selected and key != "empty"]
        if not keys:
            self.confirm(t("window.nothing_selected"), color=SUB)
            return
        depth = {key: len(list(ancestors(tree, key))) for key in keys}
        top = min(depth.values())
        text = "\n".join("  " * (depth[key] - top) + self.row_as_text(tree, key) for key in keys)
        self.to_clipboard(text, t("window.one_row_copied") if len(keys) == 1
                          else t("window.rows_copied", n=len(keys)))

    def select_all(self):
        """Ctrl+A: all visible rows of the tab (collapsed ones are left out)."""
        tree = self.active_tree()
        if tree:
            tree.selection_set([key for key in visible_rows(tree) if key != "empty"])
        return "break"

    def context_menu(self, tree, event):
        """Right-click: copy value, copy selected rows, select all. A row that isn't selected gets selected first
        (like in Explorer)."""
        key = tree.identify_row(event.y)
        if key and key not in tree.selection():
            tree.selection_set(key)
        menu = tk.Menu(self.win, tearoff=False, bg=BG_LIST, fg=FG, activebackground=HOVER, activeforeground=FG,
                       disabledforeground=SUB, bd=0, font=FONT)
        column, value = self.copyable.get(key, (None, None))
        menu.add_command(label=t("menu.copy_value" if column == "#1" else "menu.copy_name"),
                         state="normal" if value else "disabled",
                         command=lambda: self.to_clipboard(value, t("window.copied", text=value)))
        menu.add_command(label=t("menu.copy_selected"), accelerator=t("menu.ctrl_c"), command=self.copy_selection)
        menu.add_separator()
        menu.add_command(label=t("menu.select_all"), accelerator=t("menu.ctrl_a"), command=self.select_all)
        menu.tk_popup(event.x_root, event.y_root)

    def to_clipboard(self, text, message):
        self.win.clipboard_clear()
        self.win.clipboard_append(text)
        self.confirm(message)

    def confirm(self, message, color=ACCENT):
        """Short message in the footer, after 2 s (or 4 s for errors) the hint again."""
        if self.message_job:
            self.win.after_cancel(self.message_job)
        self.foot.configure(text=("✓  " if color == ACCENT else "⚠  ") + message, foreground=color)
        self.message_job = self.win.after(2000 if color == ACCENT else 4000, self.message_end)

    def build_buttons(self, tree, categories):
        """A real "Copy" button on the right of each category row (categories: [(row key, category ID)])."""
        for button in self.buttons.pop(tree, {}).values():
            button.destroy()
        self.buttons[tree] = {}
        for key, category in categories:
            button = ttk.Button(tree, text=f"⧉  {t('hw.copy')}", style="Small.TButton", cursor="hand2",
                                command=lambda c=category: self.copy_category(c))
            button.bind("<Enter>", lambda e, c=category: self.footer(t(
                "hw.copy.overview" if self.overview() else "hw.copy.details",
                category=hardware_data.category_name(c))))
            button.bind("<Leave>", lambda e: self.footer(t(HINT)))
            self.buttons[tree][key] = button
        self.win.after_idle(self.place_buttons, tree)

    def place_buttons(self, tree):
        """Put the buttons at the height of their row – or hide them if the row isn't visible right now."""
        for key, button in self.buttons.get(tree, {}).items():
            box = tree.bbox(key) if tree.exists(key) else ""
            if box:
                _, y, _, h = box
                button.place(relx=1.0, x=-self.px(4), y=y + 1, height=h - 2, anchor="ne")
            else:
                button.place_forget()

    def overview(self):
        """True as long as the sub-tab "Overview" (only model names) is selected in the tab Hardware data."""
        return self.hw_view.get() == "overview"

    def hw_columns(self, tree):
        """Overview: only names, they get the full width (otherwise long CPU names are cut off).
        Details: field name on the left, value next to it – split like in build_tabs (300 : 260)."""
        width = max(tree.winfo_width(), self.px(560))
        if self.overview():
            tree.column("value", width=0, stretch=False)
            tree.column("#0", width=width)
        else:
            tree.column("value", width=width * 260 // 560, stretch=True)
            tree.column("#0", width=width - width * 260 // 560)

    def copy_category(self, category):
        """"Copy" in a category row: the way the category is shown right now (category: ID)."""
        devices = dict(self.hw_data or []).get(category)
        if not devices:
            return
        name = hardware_data.category_name(category)
        if self.overview():
            self.to_clipboard(hardware_data.model_names_as_text([(category, devices)]),
                              t("hw.copied.overview", category=name))
        else:
            self.to_clipboard(hardware_data.as_text(category, devices, self.private()),
                              t("hw.copied.details", category=name))

    def copy_all(self):
        """"Copy all" at the top of the tab: all categories the way the selected sub-tab shows them."""
        if not self.hw_data:
            self.confirm(t("hw.not_read_yet"), color=SUB)
        elif self.overview():
            self.to_clipboard(hardware_data.model_names_as_text(self.hw_data), t("hw.all_models_copied"))
        else:
            self.to_clipboard(hardware_data.all_as_text(self.hw_data, self.private()), t("hw.all_data_copied"))

    def message_end(self):
        self.message_job = None
        if self.error:
            self.foot.configure(text=self.error, foreground=hex_color(TRAFFIC_LIGHT["red"]))
        else:
            self.foot.configure(text=t(HINT), foreground=SUB)

    # --- Hardware data ---
    def load_hw_data(self):
        """Reads the hardware data in the background – when first shown and after every reopening of the window.
        Until then the old data stays."""
        if self.hw_data_current or self.hw_data_thread:
            return

        def read():
            try:
                self.hw_data_new = hardware_data.read_all()
            except Exception:
                logging.exception("Hardware data could not be read")
                self.hw_data_new = self.hw_data or []

        self.hw_data_thread = threading.Thread(target=read, daemon=True)
        self.hw_data_thread.start()
        self.win.after(100, self.hw_data_wait)

    def hw_data_wait(self):
        if self.hw_data_thread.is_alive():
            self.win.after(100, self.hw_data_wait)
            return
        self.hw_data_thread = None
        self.hw_data, self.hw_data_current = self.hw_data_new, True
        self.update()
