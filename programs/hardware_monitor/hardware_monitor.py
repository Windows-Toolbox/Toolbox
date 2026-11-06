"""
hardware_monitor.py – Hardware Monitor in the notification area of the taskbar.
Shows CPU and GPU temperature as two numbers; a click on one of the icons opens the detail window.

Files:
  hardware_monitor.py            this program: reading sensors, tray icons, remote control by the Toolbox
  detail_window_content.py       WHAT the detail window shows (tabs, values) – this is where you adjust the window
  detail_window.py               builds the window from it (normally doesn't need to be touched)
  hardware_monitor_settings.py   defaults, loading and saving – shared with the page in the Toolbox

Requirements (Setup.exe sets everything up):
  - PawnIO installed:              winget install namazso.PawnIO
  - LibreHardwareMonitor release (LibreHardwareMonitor.zip) extracted to lhm/ in this folder
  - Python packages from requirements.txt
  - start with admin rights (otherwise CPU and motherboard values are missing)

Usage:
  pythonw hardware_monitor.py          -> tray icons (without a console window)
  python  hardware_monitor.py --list   -> lists all sensors (for adjusting the names)

Economical in the background: as long as the window is closed, only CPU and GPU are read, the GPU temperature of
Nvidia cards directly via nvml.dll instead of LHM (a fraction of the effort). At the start only CPU and GPU are
opened first so that the icons are there right away; the remaining hardware loads afterwards in the background.
The refresh rates can be set by right-clicking an icon; all other settings via the Toolbox GUI. Saved in
data/hardware_monitor.json. The GUI controls the program via a local connection (127.0.0.1, port IPC_PORT).

Errors end up in data/logs/hardware_monitor.log (pythonw has no console).
"""

import ctypes
import json
import logging
import os
import queue
import re
import socket
import sys
import threading
import time
from pathlib import Path

sys.dont_write_bytecode = True                              # don't create __pycache__ folders (like the Toolbox)
sys.path.insert(0, str(Path(__file__).resolve().parent))    # find detail_window – also when started with python -m
from detail_window import ICON_FILE, SENSOR_TYPES, DetailWindow, fmt_value, temp_color  # noqa: E402
# Defaults, rates, port, loading/saving – shared with the page in the Toolbox
from hardware_monitor_settings import (ICON_RATES, IPC_PORT, LOG_FILE, POPUP_RATES, load_settings,  # noqa: E402
                                       save_settings, short_num, t)
import i18n  # noqa: E402  (language – lives in toolbox\, hardware_monitor_settings adds the path)
import energy  # noqa: E402  (power consumption: estimate, log, add up)
import fans  # noqa: E402  (fan control: curves for graphics card and case fans)


# ---------- Settings ----------
# Folder with LibreHardwareMonitorLib.dll (created by Setup.exe, see "downloads" in program.json)
LHM_DIR = str(Path(__file__).resolve().parent / "lhm")

# With "auto": which sensor is shown in the tray icon – the first one from the list that exists
CPU_SENSORS = ("Core (Tctl/Tdie)", "CPU Package", "Core Average")
GPU_SENSORS = ("GPU Core", "GPU Hot Spot")

# Which hardware is read. Whatever you don't need, simply take it out.
# At the start only CPU and GPU – that's enough for the icons and takes only a fraction of a second.
FAST_HARDWARE = ("IsCpuEnabled", "IsGpuEnabled")
# The rest is loaded afterwards in the background: the RAM alone needs ~8 s when opening
# (LHM reads the data of the memory modules via the SMBus).
LATER_HARDWARE = (
    "IsMemoryEnabled", "IsMotherboardEnabled", "IsControllerEnabled", "IsStorageEnabled",
    "IsNetworkEnabled", "IsPsuEnabled", "IsBatteryEnabled", "IsPowerMonitorEnabled",
)

logging.basicConfig(
    filename=LOG_FILE,
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
)

# ---------- Loading LibreHardwareMonitor ----------
sys.path.append(LHM_DIR)
import clr  # pythonnet  # noqa: E402
clr.AddReference("LibreHardwareMonitorLib")
from LibreHardwareMonitor.Hardware import Computer, HardwareType, SensorType  # noqa: E402


def enable_hardware(props):
    """Switches on hardware groups. After pc.Open() LHM adds the devices right away.

    Deliberately via SetValue instead of `pc.IsMemoryEnabled = True`: pythonnet only releases the Python lock (GIL)
    during .NET method calls, not during assignments. Otherwise all other threads would stand still during the ~8 s
    for the RAM – clicks on the icon, the window and the status query of the Toolbox.
    """
    for prop in props:
        info = pc.GetType().GetProperty(prop)
        if info is None:                 # older LHM versions don't know all switches
            logging.warning("Hardware group %s doesn't exist in this LHM version", prop)
            continue
        try:
            info.SetValue(pc, True)
        except Exception:
            logging.exception("Hardware group %s could not be switched on", prop)


pc = Computer()
enable_hardware(FAST_HARDWARE)
pc.Open()


# ---------- GPU temperature directly from the Nvidia driver ----------
class NvidiaTemp:
    """Reads the GPU temperature via nvml.dll (part of the Nvidia driver, lives in System32).

    Much more economical for the icons than LHM: LHM updates all GPU values on every update (~90 ms),
    NVML delivers only the one temperature (a fraction of a millisecond). It's the same value as "GPU Core".
    Without an Nvidia card, available is False and everything runs via LHM as before.
    """
    NVML_TEMPERATURE_GPU = 0

    def __init__(self):
        self.available = False
        try:
            self.dll = ctypes.WinDLL("nvml.dll")
            self.handle = ctypes.c_void_p()
            if (self.dll.nvmlInit_v2() == 0                              # 0 = NVML_SUCCESS
                    and self.dll.nvmlDeviceGetHandleByIndex_v2(0, ctypes.byref(self.handle)) == 0):
                self.available = True                                    # first Nvidia card
        except (OSError, AttributeError):                                # no nvml.dll -> no Nvidia card
            pass

    def read(self):
        temp = ctypes.c_uint()
        if self.dll.nvmlDeviceGetTemperature(self.handle, self.NVML_TEMPERATURE_GPU, ctypes.byref(temp)) != 0:
            return None
        return float(temp.value)

    def power(self):
        """Power draw of the whole graphics card in watts (NVML reports milliwatts) – or None."""
        milliwatts = ctypes.c_uint()
        if self.dll.nvmlDeviceGetPowerUsage(self.handle, ctypes.byref(milliwatts)) != 0:
            return None
        return milliwatts.value / 1000

    def close(self):
        if self.available:
            self.dll.nvmlShutdown()


nvidia = NvidiaTemp()


def enum_table(enum, names):
    """Translates .NET enum values into their names as text (only those that exist in this LHM version)."""
    return [(getattr(enum, n), n) for n in names if hasattr(enum, n)]


def enum_name(value, table):
    for enum_value, name in table:
        if value == enum_value:
            return name
    return str(value)


HW_TYPES = enum_table(HardwareType, (
    "Motherboard", "SuperIO", "Cpu", "Memory", "GpuNvidia", "GpuAmd", "GpuIntel", "Storage",
    "Network", "Cooler", "EmbeddedController", "Psu", "Battery", "PowerMonitor"))

SENSOR_TYPE_TABLE = enum_table(SensorType, SENSOR_TYPES)

# Hardware type of LHM -> device as it's called in detail_window_content.py for Group(...) (everything else: "other")
HW_DEVICE = {
    "Cpu": "cpu", "GpuNvidia": "gpu", "GpuAmd": "gpu", "GpuIntel": "gpu", "Memory": "ram",
    "Motherboard": "motherboard", "SuperIO": "motherboard", "EmbeddedController": "motherboard",
    "Storage": "drive", "Network": "network", "Battery": "battery",
}


def walk(hw, depth=0):
    """Yields a device and all its sub-devices (e.g. the sensor chip on the motherboard), each updated."""
    hw.Update()
    yield hw, depth
    for sub in hw.SubHardware:
        yield from walk(sub, depth + 1)


def pick(hardware, kind, priority):
    """Chooses the temperature sensor that comes first in `priority`."""
    for wanted in priority:
        for hw in hardware:
            if hw["kind"] != kind:
                continue
            for s in hw["sensors"]:
                if s["type"] == "Temperature" and s["name"] == wanted:
                    return {"hw": hw["id"], "sensor": s["id"], "value": s["value"]}
    return None


def collect(devices=None, icons=("cpu", "gpu")):
    """Reads the sensors.

    devices: which devices are read, e.g. {"gpu", "drive"} (names as in HW_DEVICE); None = all.
             The devices from icons are always read, their temperature is in the icon. Only for the icons:
             collect(set()).
    icons:   which icons exist – without a GPU icon (no GPU temperature) the GPU doesn't have to be read.

    Result (a "snapshot"):
      hardware: [{"id", "device", "kind" ("cpu"/"gpu"/None), "name",
                  "sensors": [{"id", "type", "name", "value"}, ...]}, ...]
      cpu / gpu: the temperature sensor chosen for the tray icon (or None)
      devices:   which devices were actually read (None = all)
    """
    # If the GPU isn't needed, NVML is enough for the GPU icon – if it shows "GPU Core" (the case with "auto").
    # Other sensors (Hot Spot, Memory Junction, …) NVML doesn't know -> then via LHM as before.
    use_nvml = devices is not None and "gpu" not in devices and "gpu" in icons and nvml_for_gpu_icon()
    if devices is not None:
        devices = set(devices) | (set(icons) - ({"gpu"} if use_nvml else set()))
    hardware = []
    for top in pc.Hardware:
        top_type = enum_name(top.HardwareType, HW_TYPES)
        device = HW_DEVICE.get(top_type, "other")
        if devices is not None and device not in devices:
            continue                             # don't even address hardware that isn't needed
        kind = device if device in ("cpu", "gpu") else None
        try:
            for hw, depth in walk(top):
                sensors = [
                    {"id": str(s.Identifier), "type": enum_name(s.SensorType, SENSOR_TYPE_TABLE),
                     "name": str(s.Name), "value": float(s.Value)}
                    for s in hw.Sensors if s.Value is not None
                ]
                if not sensors:
                    continue
                if top_type == "Network" and all(s["value"] == 0 for s in sensors):
                    continue                     # leave out unused network adapters (Bluetooth, VPN, …)
                hardware.append({
                    "id": str(hw.Identifier),
                    "device": device,
                    "kind": kind if depth == 0 else None,
                    "name": str(hw.Name) if depth == 0 else f"{top.Name} – {hw.Name}",
                    "sensors": sensors,
                })
        except Exception:
            logging.exception("Error while reading %s", top.Name)

    if use_nvml:
        gpu = {"hw": None, "sensor": None, "value": nvidia.read()}
    else:
        gpu = pick(hardware, "gpu", sensor_priority("gpu_sensor", GPU_SENSORS))
    return {
        "hardware": hardware,
        "cpu": pick(hardware, "cpu", sensor_priority("cpu_sensor", CPU_SENSORS)),
        "gpu": gpu,
        "devices": devices,
    }


def sensor_priority(key, defaults):
    """Chosen sensor first; if it doesn't exist, the automatic order applies."""
    chosen = SETTINGS.get(key, "auto")
    return defaults if chosen == "auto" else (chosen,) + defaults


# ---------- Only the two icon values ----------
ICON_SENSORS = {}        # "cpu"/"gpu" -> (device, sensor) as .NET objects: searched once, then read directly


def nvml_for_gpu_icon():
    """GPU icon via NVML instead of LHM – only works for "GPU Core" (the case with "auto"), NVML doesn't know other
    sensors."""
    return nvidia.available and SETTINGS["gpu_sensor"] in ("auto", "GPU Core")


def find_icon_sensor(kind, priority):
    """Like pick(), but delivers the .NET objects (device, sensor) instead of a value – or None."""
    devices = [hw for hw in pc.Hardware if HW_DEVICE.get(enum_name(hw.HardwareType, HW_TYPES)) == kind]
    for hw in devices:
        hw.Update()                              # some sensors only exist after the first update
    for wanted in priority:
        for hw in devices:
            for s in hw.Sensors:
                if s.SensorType == SensorType.Temperature and str(s.Name) == wanted and s.Value is not None:
                    return hw, s
    return None


def gpu_has_temperature():
    """Is there a GPU temperature? Intel onboard graphics (e.g. Iris Xe) report none – then the GPU icon is left out
    instead of permanently showing "--"."""
    if nvidia.available:
        return True
    for hw in pc.Hardware:
        if HW_DEVICE.get(enum_name(hw.HardwareType, HW_TYPES)) == "gpu":
            hw.Update()                          # some sensors only exist after the first update
            if any(s.SensorType == SensorType.Temperature for s in hw.Sensors):
                return True
    return False


def icon_values(kinds=("cpu", "gpu")):
    """Only the temperatures for the icons: {"cpu": value, "gpu": value} (None = none there).

    Per device one Update() and a single value read. collect() would query every CPU sensor (clock, load,
    power, voltage per core – around 50 on a 6-core CPU) one by one via pythonnet.
    kinds: which icons exist (without a GPU icon the GPU isn't addressed at all).
    """
    values = {}
    for kind, key, defaults in (("cpu", "cpu_sensor", CPU_SENSORS), ("gpu", "gpu_sensor", GPU_SENSORS)):
        if kind not in kinds:
            continue
        if kind == "gpu" and nvml_for_gpu_icon():
            values[kind] = nvidia.read()
            continue
        found = ICON_SENSORS.get(kind)
        if found:
            found[0].Update()
        else:
            found = find_icon_sensor(kind, sensor_priority(key, defaults))
            if found:
                ICON_SENSORS[kind] = found
        value = found[1].Value if found else None
        if value is None:
            ICON_SENSORS.pop(kind, None)         # sensor gone or (still) without a value -> search again next time
        values[kind] = None if value is None else float(value)
    return values


# ---------- Power of CPU and graphics card (for the power consumption, see energy.py) ----------
CPU_POWER = ("Package", "CPU Package")                                      # AMD, Intel
GPU_POWER = ("GPU Board Power", "GPU Package", "GPU PPT", "GPU Power")      # whole card first
POWER_SENSORS = {}       # "cpu"/"gpu"/"battery" -> (device, sensor) as .NET objects, searched once


def integrated_gpu(name):
    """Graphics in the processor – its power is already part of the CPU package power. Recognized by the name:
    Intel except Arc A/B cards, AMD "Radeon(TM) Graphics" / "Radeon 780M" / "Vega 8 Graphics"."""
    return bool(re.search(r"Intel(?!.*Arc\S* [AB]\d)|Radeon\(TM\) Graphics|Radeon \d{3}M|Vega \d+ Graphics", name))


def find_power_sensor(kind):
    """(device, sensor) for "cpu", "gpu" (dedicated only) or "battery" (discharge rate) – or None."""
    names = {"cpu": CPU_POWER, "gpu": GPU_POWER, "battery": ("Discharge Rate",)}[kind]
    for hw in pc.Hardware:
        if HW_DEVICE.get(enum_name(hw.HardwareType, HW_TYPES)) != kind:
            continue
        if kind == "gpu" and integrated_gpu(str(hw.Name)):
            continue
        hw.Update()
        for wanted in names:
            for s in hw.Sensors:
                if s.SensorType == SensorType.Power and str(s.Name) == wanted:
                    return hw, s
    return None


def power_values():
    """{"cpu": W, "gpu": W, "battery": W} – None where there's no value. CPU and GPU have just been updated by the
    measurement for the icons (or the window), so only the value is read. The battery is updated here (cheap,
    only on laptops); while it's discharging, its discharge rate is what the whole laptop draws."""
    values = {}
    for kind in ("cpu", "gpu", "battery"):
        if kind == "gpu" and nvidia.available:
            values[kind] = nvidia.power()
            continue
        if kind not in POWER_SENSORS:
            POWER_SENSORS[kind] = find_power_sensor(kind)
            if kind == "battery" and POWER_SENSORS[kind] is None and any(
                    HW_DEVICE.get(enum_name(hw.HardwareType, HW_TYPES)) == "battery" for hw in pc.Hardware):
                del POWER_SENSORS[kind]          # laptop on the charger: the discharge rate appears only on battery
        found = POWER_SENSORS.get(kind)
        if not found:
            values[kind] = None
            continue
        if kind == "battery":
            found[0].Update()
        value = found[1].Value
        values[kind] = None if value is None else float(value)
    if not values["battery"]:                    # 0 or None = charging or on the charger
        values["battery"] = None
    return values


# ---------- Fans that can be controlled (see fans.py) ----------
def find_fans():
    """One item per graphics card (all its fans together, like in Afterburner) and one per motherboard fan header –
    whatever LHM can control (a Control sensor with .Control). Call only after LATER_HARDWARE is loaded."""
    items = []
    for top in pc.Hardware:
        device = HW_DEVICE.get(enum_name(top.HardwareType, HW_TYPES))
        if device not in ("gpu", "motherboard"):
            continue
        for hw, _ in walk(top):
            controls = [s for s in hw.Sensors if s.SensorType == SensorType.Control and s.Control is not None]
            speeds = [s for s in hw.Sensors if s.SensorType == SensorType.Fan]
            if not controls:
                continue
            if device == "gpu":
                items.append(fans.FanItem(
                    id=str(hw.Identifier), kind="gpu", name=str(top.Name), controls=[s.Control for s in controls],
                    control_ids=[str(s.Identifier) for s in controls], fan_ids=[str(s.Identifier) for s in speeds],
                    minimum=max(float(s.Control.MinSoftwareValue) for s in controls),
                    maximum=min(float(s.Control.MaxSoftwareValue) for s in controls)))
                continue
            for s in controls:                   # "Fan Control #2" belongs to the speed "Fan #2"
                name = re.sub(r"\s*\bControl\b", "", str(s.Name))
                items.append(fans.FanItem(
                    id=str(s.Identifier), kind="case", name=name, controls=[s.Control],
                    control_ids=[str(s.Identifier)], fan_ids=[str(f.Identifier) for f in speeds if str(f.Name) == name],
                    minimum=float(s.Control.MinSoftwareValue), maximum=float(s.Control.MaxSoftwareValue)))
    return items


# ---------- Drive health (like CrystalDiskInfo) ----------
HEALTH_EVERY = 10                # seconds between two SMART readings while the tab "Drive health" is shown
CAUTION_IDS = {0x05, 0xC5, 0xC6}  # reallocated / pending / uncorrectable sectors: raw > 0 = caution (like CrystalDiskInfo)


def attribute_status(attribute, nvme, ssd):
    """"good", "caution" or "bad" for one SMART attribute – the rules of CrystalDiskInfo: below its threshold = bad;
    on hard disks reallocated / pending / uncorrectable sectors = caution; NVMe: critical warning = bad, data
    integrity errors = caution (the spare is checked in read_drive_health)."""
    a, kind = attribute.Attribute, str(attribute.Info.Type)
    raw = int(a.RawValueULong)
    if nvme:
        if kind == "CriticalWarning" and raw:
            return "bad"
        return "caution" if kind == "MediaAndDataIntegrityErrors" and raw else "good"
    if a.Threshold and a.CurrentValue < a.Threshold:
        return "bad"
    return "caution" if not ssd and a.ID in CAUTION_IDS and raw else "good"


def read_drive_health():
    """Health of all drives from DiskInfoToolkit (the library LHM reads drives with – a port of CrystalDiskInfo):
    [{"number", "model", "firmware", "serial", "size", "bus", "nvme", "ssd", "status" ("Good"/"Caution"/"Bad"/
    "Unknown"), "temp", "temp_warning", "temp_critical", "life", "hours", "count", "reads", "writes",
    "attributes": [{"id", "name", "current", "worst", "threshold", "raw", "status"}]}]. Needs admin rights."""
    try:
        clr.AddReference("DiskInfoToolkit")
        from DiskInfoToolkit import StorageManager
    except Exception:
        logging.exception("DiskInfoToolkit could not be loaded – no drive health")
        return []

    def number(value):
        return None if value is None else float(value)

    drives = []
    for s in StorageManager.Storages:
        try:
            s.Update()
            smart = s.Smart
            nvme, ssd = bool(s.IsNVMe), bool(s.IsSSD)
            attributes = [{
                # NVMe values have no ID of their own – numbered like CrystalDiskInfo does (01, 02, …)
                "id": i + 1 if nvme else int(a.Attribute.ID), "name": str(a.Info.Name),
                "kind": str(a.Info.Type), "current": int(a.Attribute.CurrentValue),
                "worst": int(a.Attribute.WorstValue), "threshold": int(a.Attribute.Threshold),
                "raw": int(a.Attribute.RawValueULong), "status": attribute_status(a, nvme, ssd),
            } for i, a in enumerate(smart.SmartAttributes)]
            raw = {a["kind"]: a["raw"] for a in attributes}
            if nvme and raw.get("AvailableSpare", 100) < raw.get("AvailableSpareThreshold", 0):
                for a in attributes:
                    if a["kind"] == "AvailableSpare":
                        a["status"] = "caution"
            drives.append({
                "number": int(s.DriveNumber), "model": str(s.Model or "").strip(),
                "firmware": str(s.Firmware or "").strip(), "serial": str(s.SerialNumber or "").strip(),
                "size": float(s.TotalSize), "bus": str(s.BusType), "nvme": nvme, "ssd": ssd,
                "status": str(smart.DiskStatus), "temp": number(smart.Temperature),
                "temp_warning": number(smart.TemperatureWarning), "temp_critical": number(smart.TemperatureCritical),
                "life": number(smart.Life),
                "hours": int(smart.MeasuredPowerOnHours or smart.DetectedPowerOnHours),
                "count": int(smart.PowerOnCount), "reads": number(smart.HostReads),
                "writes": number(smart.HostWrites), "attributes": attributes,
            })
        except Exception:
            logging.exception("Drive health of %s could not be read", getattr(s, "Model", "?"))
    return sorted(drives, key=lambda d: d["number"])


# ---------- Settings ----------
SETTINGS = load_settings()       # updated in place on "reload" (from the GUI)


def store_settings():
    try:
        save_settings(SETTINGS)
    except OSError:
        logging.exception("Settings could not be saved")


def rate_label(seconds):
    if seconds == 1:
        return t("menu.one_second")
    return t("menu.seconds", n=short_num(seconds))


if "--list" in sys.argv:
    enable_hardware(LATER_HARDWARE)
    collect()            # discard the first measurement: some sensors still deliver 0 right after Open()
    time.sleep(1)
    print("For detail_window_content.py:  [device] -> Group(\"device\", …)   type and name -> "
          "Value(\"Name\", type=\"Type\")")
    for hw in collect()["hardware"]:
        print(f"\n[{hw['device']}] {hw['name']}")
        for s in hw["sensors"]:
            print(f"    {s['type']:16} {s['name']:32} {fmt_value(s['type'], s['value'])}")
    pc.Close()
    sys.exit()


# ---------- From here on: tray icons and program ----------
import tkinter as tk  # noqa: E402
from PIL import Image, ImageDraw, ImageFont  # noqa: E402
import pystray  # noqa: E402

STRIPE = {"cpu": (70, 140, 255), "gpu": (118, 185, 0)}     # color stripe: blue = CPU, green = GPU
EMPTY = {"hardware": [], "cpu": None, "gpu": None, "devices": set()}    # before the first measurement for the window


FONT_BIG = ImageFont.truetype("arialbd.ttf", 44)     # Arial bold, for 2 characters
FONT_SMALL = ImageFont.truetype("arialbd.ttf", 31)   # for 3 characters, e.g. "100"


def make_icon(kind, value):
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    text = "--" if value is None else f"{value:.0f}"
    font = FONT_BIG if len(text) <= 2 else FONT_SMALL
    d.text((32, 27), text, font=font, fill=temp_color(value, SETTINGS, kind), anchor="mm")
    d.rounded_rectangle([8, 55, 56, 63], radius=4, fill=STRIPE[kind])
    return img


def make_logo(kind):
    """Program logo with the color stripe of CPU or GPU – only for the taskbar settings, see TrayIcon."""
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    img.alpha_composite(Image.open(ICON_FILE).convert("RGBA").resize((50, 50), Image.LANCZOS), (7, 0))
    ImageDraw.Draw(img).rounded_rectangle([8, 55, 56, 63], radius=4, fill=STRIPE[kind])
    return img


class TrayIcon(pystray.Icon):
    """pystray icon that appears in the taskbar settings with the logo and its own entry.

    Windows lists every tray icon under Settings > Personalization > Taskbar > "Other system tray icons"
    (stored in HKCU\\Control Panel\\NotifyIconSettings) and recognizes it by .exe + number (uID). For that two
    methods of pystray (version pinned in requirements.txt) are replaced:
      - _show: Windows remembers the image from the registration and doesn't take over later changes. That's why it
        always registers with the logo and sets the actual image right after – also when pystray re-registers the
        icon by itself after Explorer restarts (otherwise the temperature would be stored there).
      - _message: pystray sends the number under the wrong name (hID instead of uID), so it's always 0 –
        CPU and GPU would be one and the same icon for Windows.
    """
    UIDS = {"cpu": 1, "gpu": 2}                  # fixed per kind – stays the same even if the order changes

    def __init__(self, kind, menu):
        self.uid = self.UIDS[kind]
        self.logo = make_logo(kind)
        super().__init__(f"hardware_monitor_{kind}", make_icon(kind, None), kind.upper(), menu)

    def _show(self):
        image = self.icon
        self._icon = self.logo                   # directly, not via self.icon – that would already send NIM_MODIFY
        self._release_icon()
        super()._show()                          # NIM_ADD with the logo
        self._icon = image
        self._update_icon()                      # NIM_MODIFY with the actual image

    def _message(self, code, flags, **kwargs):
        from pystray._util import win32
        win32.Shell_NotifyIcon(code, win32.NOTIFYICONDATAW(
            cbSize=ctypes.sizeof(win32.NOTIFYICONDATAW), hWnd=self._hwnd, uID=self.uid, uFlags=flags, **kwargs))


# ---------- Windows helpers ----------
def set_dpi_aware():
    """Otherwise Windows renders the window blurry at 125/150 % scaling."""
    ctypes.windll.shcore.SetProcessDpiAwareness(1)


def set_efficiency_mode(on):
    """Switch the Windows 11 efficiency mode (leaf symbol in Task Manager) on or off.

    On: Windows schedules the process power-efficiently (EcoQoS – low clock, on hybrid CPUs the efficiency
    cores) and with the lowest priority. That's enough for the icons in the background; with the window open it
    is switched off so that the interface stays smooth.
    """
    class PROCESS_POWER_THROTTLING_STATE(ctypes.Structure):
        _fields_ = [("Version", ctypes.c_ulong), ("ControlMask", ctypes.c_ulong), ("StateMask", ctypes.c_ulong)]

    throttle = 0x1                                  # PROCESS_POWER_THROTTLING_EXECUTION_SPEED
    kernel32 = ctypes.windll.kernel32
    process = ctypes.c_void_p(-1)                   # pseudo handle for the own process (GetCurrentProcess)
    state = PROCESS_POWER_THROTTLING_STATE(1, throttle, throttle if on else 0)
    # Older Windows versions don't know PowerThrottling – then the call only returns False, no error
    kernel32.SetProcessInformation(process, 4, ctypes.byref(state), ctypes.sizeof(state))  # 4 = PowerThrottling
    kernel32.SetPriorityClass(process, 0x40 if on else 0x20)                    # IDLE_ or NORMAL_PRIORITY_CLASS


_mutex = None                    # has to live until the program ends – otherwise Windows releases the mutex again


def already_running():
    """True if the program is already running (important in case a launcher starts it twice)."""
    global _mutex
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _mutex = kernel32.CreateMutexW(None, False, "Local\\HardwareMonitor_SingleInstance")
    return ctypes.get_last_error() == 183               # ERROR_ALREADY_EXISTS


# ---------- Program ----------
class HardwareMonitorApp:
    def __init__(self):
        self.root = tk.Tk()
        self.root.withdraw()                     # main window invisible, only the detail window is shown
        self.window = None                       # detail window – only built the first time it opens
        self.window_wants = False                # what the window shows right now: False = closed, otherwise devices (None = all)
        self.hardware_ready = False              # True as soon as the remaining hardware is loaded
        self.present = None                      # devices with sensors on this PC, e.g. {"cpu", "gpu", "battery"}

        self.stop = threading.Event()
        self.wake = threading.Event()            # wakes the measuring thread early (window opened, rate changed)
        self.lock = threading.Lock()
        self.latest = EMPTY                      # last measurement for the window
        self.shown = {}                          # what's in the icons right now -> only redraw on change
        self.ui_queue = queue.Queue()            # messages from the other threads to tkinter
        self.settings = SETTINGS                 # the same settings as everywhere in the script
        self.order = list(self.settings["icon_order"])
        # Icons that really exist: without GPU temperature (Intel onboard graphics) no GPU icon
        self.kinds = [kind for kind in self.order if kind != "gpu" or gpu_has_temperature()]
        if "gpu" not in self.kinds:
            logging.info("No GPU temperature found – the GPU icon is not shown.")
        self.ready = {kind: threading.Event() for kind in self.kinds}
        self.current = {"cpu": None, "gpu": None}    # current icon values, for the status query of the GUI
        self.energy = energy.EnergyLog()         # logged power consumption (only while switched on)
        self.power_now = None                    # last power measurement: {"cpu", "gpu", "battery", "watts"} or None
        self.fans = fans.FanController()         # fan curves – only the measuring thread talks to the fans
        self.drive_health = None                 # last reading for the tab "Drive health" (None = not read yet)
        self.next_health = 0.0                   # time.monotonic() of the next SMART reading
        self.health_until = 0.0                  # also read until then (requested via IPC "drive_health")

        self.icons = {kind: TrayIcon(kind, self.make_menu()) for kind in self.kinds}
        self.sensor_thread = threading.Thread(target=self.sensor_loop, daemon=True)
        self.ipc_thread = threading.Thread(target=self.ipc_server, daemon=True)

    # --- Right-click menu ---
    def make_menu(self):
        return pystray.Menu(
            # texts as functions: pystray asks for them again on every update_menu() – so a language switch works
            # right away
            pystray.MenuItem(lambda item: t("menu.details"), lambda icon, item: self.ui_queue.put("toggle"),
                             default=True),
            pystray.MenuItem(lambda item: t("menu.privacy"), lambda icon, item: self.ui_queue.put("privacy"),
                             checked=lambda item: self.settings["privacy_mode"]),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(lambda item: t("menu.icons", rate=rate_label(self.settings["icon_interval"])),
                             self.rate_menu("icon_interval", ICON_RATES)),
            pystray.MenuItem(lambda item: t("menu.window", rate=rate_label(self.settings["popup_interval"])),
                             self.rate_menu("popup_interval", POPUP_RATES)),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(lambda item: f"✕  {t('menu.quit')}", lambda icon, item: self.ui_queue.put("quit")),
        )

    def rate_menu(self, key, rates):
        """Submenu with radio items; the currently chosen one is checked."""
        # pystray counts the parameters of an action and accepts exactly (icon, item) –
        # hence small factory functions instead of lambdas with a default value (s=seconds).
        def action(seconds):
            return lambda icon, item: self.set_rate(key, seconds)

        def checked(seconds):
            return lambda item: self.settings[key] == seconds

        def label(seconds):
            return lambda item: rate_label(seconds)

        return pystray.Menu(*[
            pystray.MenuItem(label(s), action(s), checked=checked(s), radio=True)
            for s in rates
        ])

    def set_rate(self, key, seconds):
        self.settings[key] = seconds
        store_settings()
        for icon in self.icons.values():
            icon.update_menu()                   # refresh the check mark and label in both menus
        self.wake.set()                          # take over the new rate right away

    # --- Privacy mode ---
    def toggle_privacy(self):
        """Switch privacy mode on/off (switch in the window or right-click menu) – runs in the tkinter thread."""
        self.settings["privacy_mode"] = not self.settings["privacy_mode"]
        store_settings()
        self.privacy_changed()

    def privacy_changed(self):
        for icon in self.icons.values():
            icon.update_menu()                   # check mark in both menus
        if self.window:
            self.window.privacy_changed()

    # --- Detail window ---
    def toggle_window(self):
        if self.window is None:
            self.window = DetailWindow(self.root, self)
        self.window.toggle()

    def window_needs(self, devices):
        """Reported by the window: which devices it shows right now (False = closed or minimized)."""
        if devices != self.window_wants:
            was_closed = self.window_wants is False
            self.window_wants = devices
            if (devices is False) != was_closed:
                self.apply_efficiency_mode()     # with "auto": only run economically while the window is closed
            if devices is False:                 # discard old values – otherwise briefly "Live" with an old state
                with self.lock:
                    self.latest = EMPTY
            self.wake.set()                      # measure accordingly right away

    def apply_efficiency_mode(self):
        """Efficiency mode according to the setting: "auto" only while the window is closed, "on" always, "off" never."""
        mode = self.settings["efficiency_mode"]
        set_efficiency_mode(mode == "on" or (mode == "auto" and self.window_wants is False))

    def window_geometry(self, geometry):
        """Reported by the window when it closes: size and position, e.g. "480x640+1420+380"."""
        if self.settings.get("window") != geometry:
            self.settings["window"] = geometry
            store_settings()

    def snapshot(self):
        with self.lock:
            return self.latest

    # --- Background thread: measures and updates the icons ---
    def measure(self, devices):
        try:
            snap = collect(devices, self.kinds)
        except Exception:
            logging.exception("Error while reading the sensors")
            snap = dict(EMPTY)
        snap["loading"] = not self.hardware_ready
        return snap

    def measure_icons(self):
        """Only the two icon values (more economical than measure(), see icon_values())."""
        try:
            return icon_values(self.kinds)
        except Exception:
            logging.exception("Error while reading the icon values")
            ICON_SENSORS.clear()                 # search again next time
            return {"cpu": None, "gpu": None}

    def log_power(self, needed):
        """Power consumption: measure – while logging is on or the window shows the tab Power – and add it up."""
        logging_on = self.settings["energy_logging"]
        if not logging_on:
            self.energy.pause()                  # switched on again later: start a new interval
        if not (logging_on or (needed and "power" in needed)):
            self.power_now = None
            return
        try:
            values = power_values()
        except Exception:
            logging.exception("Error while reading the power values")
            POWER_SENSORS.clear()                # search again next time
            return
        if values["battery"] is not None:        # laptop on battery: measured in total
            watts = values["battery"]
        else:
            watts = energy.wall_watts(values["cpu"], values["gpu"], self.settings)
        self.power_now = dict(values, watts=watts)
        if logging_on:
            self.energy.add(watts)

    def read_health(self, needed):
        """Drive health: every HEALTH_EVERY seconds while the tab "Drive health" is shown (or IPC asked for it)."""
        now = time.monotonic()
        if not ((needed and "health" in needed) or now < self.health_until) or now < self.next_health:
            return
        self.next_health = now + HEALTH_EVERY
        try:
            self.drive_health = read_drive_health()
        except Exception:
            logging.exception("Error while reading the drive health")

    def control_fans(self):
        """Fan curves: adjust every fan that is switched to "curve" or "fixed" (or hand it back)."""
        config = self.settings["fan_control"]
        if not self.fans.wanted(config):
            return
        try:
            self.fans.apply(config, self.current, self.settings, time.monotonic())
        except Exception:
            logging.exception("Error in the fan control – handing all fans back")
            self.fans.release_all()

    def set_fan_config(self, fan_id, entry):
        """The window changed a fan's mode, source or curve (tkinter thread) – the measuring thread applies it."""
        self.settings["fan_control"] = {**self.settings["fan_control"], fan_id: entry}
        store_settings()
        self.wake.set()

    def update_icons(self, values):
        """values: {"cpu": temperature, "gpu": temperature} – None if there is no value."""
        for kind, icon in self.icons.items():
            value = values.get(kind)
            self.current[kind] = None if value is None else round(value, 1)
            text = "--" if value is None else f"{value:.0f}"
            if self.shown.get(kind) == text:
                continue                         # same number as before -> don't redraw anything
            self.shown[kind] = text
            icon.icon = make_icon(kind, value)
            shown = "–" if value is None else f"{text} °C"
            icon.title = f"{kind.upper()}: {shown}  ·  {t('icon.click')}"

    def sensor_loop(self):
        """Window closed (or minimized): read only CPU and GPU every `icon_interval` seconds.
        Window open: every `popup_interval` seconds the devices of the selected tab (the icons get the values too)."""
        try:
            self.update_icons(self.measure_icons())          # fill the icons with real values right away …
            enable_hardware(LATER_HARDWARE)                  # … then load the rest (RAM takes ~8 s)
            snap = self.measure(None)            # "warm up" everything once: the first measurement partly delivers 0
            # With it the window hides tabs that don't exist here (e.g. motherboard on a laptop, battery on a desktop)
            self.present = {hw["device"] for hw in snap["hardware"]} or None     # measurement failed -> show all
            try:
                self.fans.set_items(find_fans())
            except Exception:
                logging.exception("Controllable fans could not be determined – no fan control")
            self.hardware_ready = True
            next_icon = next_window = 0.0
            while not self.stop.is_set():
                now = time.monotonic()
                needed = self.window_wants
                # while a fan is controlled, measure at least every fans.INTERVAL seconds (its temperature)
                icon_every = self.settings["icon_interval"]
                if self.fans.wanted(self.settings["fan_control"]):
                    icon_every = min(icon_every, fans.INTERVAL)

                if needed is not False and now >= next_window:
                    snap = self.measure(needed)
                    with self.lock:
                        if self.window_wants is not False:      # window not closed during the measurement
                            self.latest = snap
                    self.update_icons({k: snap[k]["value"] if snap[k] else None for k in ("cpu", "gpu")})
                    self.log_power(needed)
                    self.control_fans()
                    self.read_health(needed)
                    self.ui_queue.put("data")    # let the window redraw
                    next_window = now + self.settings["popup_interval"]
                    next_icon = now + icon_every
                elif now >= next_icon:
                    self.update_icons(self.measure_icons())
                    self.log_power(needed)
                    self.control_fans()
                    self.read_health(needed)
                    next_icon = now + icon_every

                due = min(next_icon, next_window) if needed is not False else next_icon
                self.wake.wait(max(0.05, due - time.monotonic()))
                if self.wake.is_set():           # window opened, tab changed, rate changed -> reschedule right away
                    self.wake.clear()
                    next_icon = next_window = 0.0
        finally:
            self.fans.release_all()              # hand every controlled fan back to the card / the BIOS
            pc.Close()
            nvidia.close()

    # --- Remote control by the Toolbox GUI ---
    def ipc_server(self):
        """Waits for commands from the GUI: "status", "reload" or "quit" (one command per connection).

        Needed because this program runs with admin rights and the GUI without: Windows forbids the GUI to simply
        end it – a request over a local connection works, though.
        """
        server = None
        logged = False
        while server is None and not self.stop.is_set():
            try:
                server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                server.bind(("127.0.0.1", IPC_PORT))     # only reachable from this PC
                server.listen()
            except OSError:
                server.close()
                server = None
                if not logged:                           # port still taken (e.g. right after a restart) -> try again later
                    logging.warning("Port %s taken – remote control will be tried again later", IPC_PORT)
                    logged = True
                self.stop.wait(2)
        if server is None:
            return
        # accept() waits without a timeout until someone connects – no waking up when idle. When quitting nobody has
        # to wake the thread: it runs as a daemon and ends with the program.
        with server:
            while not self.stop.is_set():
                try:
                    conn, _ = server.accept()
                except OSError:
                    break
                with conn:
                    try:
                        conn.settimeout(1)
                        command = conn.recv(64).decode("utf-8", "replace").strip()
                        reply = self.handle_command(command)
                        conn.sendall((json.dumps(reply) + "\n").encode("utf-8"))
                    except OSError:
                        pass

    def handle_command(self, command):
        if command == "status":
            power = self.power_now
            return {"ok": True, "pid": os.getpid(), "cpu": self.current["cpu"], "gpu": self.current["gpu"],
                    "order": self.order, "icons": self.kinds, "privacy_mode": self.settings["privacy_mode"],
                    "power": round(power["watts"]) if power else None,
                    "energy": energy.totals(self.energy.snapshot())}
        if command in ("reload", "quit", "show_power", "show_fans"):
            self.ui_queue.put(command)
            return {"ok": True}
        if command == "drive_health":            # for checking without a window: last reading, read again soon
            self.health_until = time.monotonic() + 30
            self.wake.set()
            return {"ok": True, "drives": self.drive_health}
        if command == "energy_delete":
            ok = self.energy.delete_all()
            self.ui_queue.put("data")
            return {"ok": ok}
        return {"ok": False, "error": f"Unknown command: {command}"}

    def reload_settings(self):
        """The GUI has changed settings -> read them again and apply them everywhere."""
        window = SETTINGS["window"]              # belongs to the window – the Toolbox may have an old state
        SETTINGS.update(load_settings())         # incl. privacy mode – window.update() below shows it
        SETTINGS["window"] = window
        new_language = i18n.reload()             # switched in the Toolbox? Menus and icons follow right below
        self.shown.clear()                       # redraw the icons (e.g. new color limits)
        ICON_SENSORS.clear()                     # search the icon sensors again (e.g. another sensor chosen)
        for icon in self.icons.values():
            icon.update_menu()
        self.apply_efficiency_mode()             # e.g. set to "Always on" in the Toolbox
        if not SETTINGS["energy_logging"]:
            self.energy.save()                   # logging switched off: save what was counted until now
        if self.window and new_language:
            self.window.language_changed()
        elif self.window:
            self.window.update()
        self.wake.set()

    # --- tkinter side ---
    def poll_queue(self):
        try:
            while True:
                message = self.ui_queue.get_nowait()
                if message == "toggle":
                    self.toggle_window()
                elif message == "privacy":
                    self.toggle_privacy()
                elif message in ("show_power", "show_fans"):     # buttons on the Toolbox page
                    if self.window is None:
                        self.window = DetailWindow(self.root, self)
                    self.window.show(tab="tab.power" if message == "show_power" else "tab.fans")
                elif message == "data" and self.window:
                    self.window.update()
                elif message == "reload":
                    self.reload_settings()
                elif message == "quit":
                    self.quit()
                    return
        except queue.Empty:
            pass
        # window closed: look less often (a click on the icon then reacts at most ¼ s later)
        self.root.after(100 if self.window_wants is not False else 250, self.poll_queue)

    def quit(self):
        self.stop.set()
        self.wake.set()                          # don't let the measuring thread sleep it out first
        if self.window:
            self.window.save_geometry()          # remember position and size
        self.energy.save()                       # logged power consumption since the last save
        for icon in self.icons.values():
            icon.stop()
        self.root.quit()

    def run(self):
        # register the icons one after another so the order in the taskbar is always the same
        for kind in self.kinds:
            self.icons[kind].run_detached(setup=lambda ic, k=kind: (setattr(ic, "visible", True), self.ready[k].set()))
            self.ready[kind].wait(timeout=5)
        self.apply_efficiency_mode()             # the window is closed – so with "auto" on until it opens
        self.sensor_thread.start()
        self.ipc_thread.start()
        self.root.after(100, self.poll_queue)
        try:
            self.root.mainloop()                 # runs until quit() is called
        finally:                                 # also after an error: end the measuring thread – it hands the fans back
            self.stop.set()
            self.wake.set()
            self.sensor_thread.join(timeout=5)
        self.root.destroy()


def main():
    if already_running():
        logging.info("Already running – second start is ending.")
        pc.Close()
        return
    set_dpi_aware()
    try:
        HardwareMonitorApp().run()
    except Exception:
        logging.exception("Unexpected error")
        raise


if __name__ == "__main__":
    main()
