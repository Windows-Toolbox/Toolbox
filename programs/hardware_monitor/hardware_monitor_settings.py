"""
What hardware_monitor.py and its page in the Toolbox (hardware_monitor_gui.py) both need: defaults, allowed values,
loading and saving data\\hardware_monitor.json, port and name of the task – and t, the texts from i18n.json (see
toolbox\\i18n.py), plus number and date formatting in the chosen language.

A separate file because the Toolbox can't import hardware_monitor.py – that loads LibreHardwareMonitor on import.
Port and task are in program.json because setup\\setup.ps1 needs them too.
"""

import json
import re
import sys
from pathlib import Path

FOLDER = Path(__file__).resolve().parent
sys.path.append(str(FOLDER.parents[1] / "toolbox"))     # storage.py and i18n.py (shared with the Toolbox)
import storage  # noqa: E402
import i18n  # noqa: E402

MANIFEST = json.loads((FOLDER / "program.json").read_text(encoding="utf-8"))
IPC_PORT = MANIFEST["port"]            # local remote control by the Toolbox (only reachable from this PC)
TASK = MANIFEST["task"]                # scheduled task that starts the monitor at sign-in with admin rights
SETTINGS_FILE = storage.settings_file(MANIFEST["id"])
LOG_FILE = storage.log_file(MANIFEST["id"])
t = i18n.Texts(FOLDER / "i18n.json")   # all texts of the Hardware Monitor: t("id") in the chosen language

# Choices in the right-click menu and in the Toolbox (seconds)
ICON_RATES = (1, 2, 5, 10)      # taskbar icons (reads only CPU and GPU)
POPUP_RATES = (0.5, 1, 2, 5)    # open window (reads the devices of the selected tab)
EFFICIENCY_MODES = ("auto", "on", "off")

# Defaults. Changed via the right-click menu or the Toolbox; both end up in data\hardware_monitor.json.
DEFAULT_SETTINGS = {
    "icon_interval": 2,
    "popup_interval": 1,
    "icon_order": ["cpu", "gpu"],    # order in the taskbar (takes effect after restarting the program)
    "cpu_sensor": "auto",            # "auto" = first existing one from CPU_SENSORS, otherwise a sensor name
    "gpu_sensor": "auto",
    # Traffic-light color of the temperatures: yellow from warn, red from hot – separate, because CPUs may get
    # considerably hotter under load (Ryzen and Intel only throttle at about 90–100 °C, graphics cards mostly from
    # about 83 °C).
    "cpu_warn_temp": 75,
    "cpu_hot_temp": 90,
    "gpu_warn_temp": 70,
    "gpu_hot_temp": 83,
    "window": "",                    # size and position of the detail window, e.g. "480x640+1420+380" (set by itself)
    "efficiency_mode": "auto",       # "auto" = only while the window is closed, "on" = always, "off" = never
    "privacy_mode": False,           # hide serial numbers, MAC addresses, processor ID and computer name (████████)
    # Power consumption of the whole PC (estimated, see energy.py) – logged only while switched on
    "energy_logging": False,
    "energy_price": 0.35,            # price per kWh (average – no dynamic prices)
    "energy_currency": "€",
    "energy_other_watts": 40,        # motherboard, RAM, drives, fans, USB devices – everything except CPU and GPU
    "energy_psu_efficiency": 87,     # % – the power supply loses the rest as heat (80 PLUS Bronze/Gold: about 85–90 %)
    # Fan control per fan (see fans.py): {LHM ID: {"mode", "source", "curve", "fixed"}} – missing = "auto"
    "fan_control": {},
}
OTHER_WATTS = (0, 300)               # allowed range of energy_other_watts
PSU_EFFICIENCY = (50, 100)           # allowed range of energy_psu_efficiency

# Until November 2026 some keys and values had German names – load_settings() takes them over once
OLD_KEYS = {"effizienzmodus": "efficiency_mode"}
OLD_VALUES = {"efficiency_mode": {"an": "on", "aus": "off"}}


def load_settings():
    """Reads the settings; everything missing or invalid gets the default."""
    raw = storage.load(SETTINGS_FILE, {})
    s = storage.load(SETTINGS_FILE, DEFAULT_SETTINGS)
    for old, new in OLD_KEYS.items():
        value = s.pop(old, None)
        if value is not None and new not in raw:
            s[new] = OLD_VALUES.get(new, {}).get(value, value)
    start = DEFAULT_SETTINGS
    if s["icon_interval"] not in ICON_RATES:
        s["icon_interval"] = start["icon_interval"]
    if s["popup_interval"] not in POPUP_RATES:
        s["popup_interval"] = start["popup_interval"]
    if not isinstance(s["icon_order"], list) or sorted(s["icon_order"]) != ["cpu", "gpu"]:
        s["icon_order"] = list(start["icon_order"])
    for key in ("cpu_sensor", "gpu_sensor"):
        s[key] = s[key].strip() if isinstance(s[key], str) and s[key].strip() else start[key]
    for kind in ("cpu", "gpu"):
        warn, hot = s[f"{kind}_warn_temp"], s[f"{kind}_hot_temp"]
        if not (isinstance(warn, (int, float)) and isinstance(hot, (int, float)) and 20 <= warn < hot <= 120):
            s[f"{kind}_warn_temp"], s[f"{kind}_hot_temp"] = start[f"{kind}_warn_temp"], start[f"{kind}_hot_temp"]
    s.pop("warn_temp", None)                     # once one traffic light for both – replaced by the values above
    s.pop("hot_temp", None)
    if not (isinstance(s["window"], str) and re.fullmatch(r"\d+x\d+\+-?\d+\+-?\d+", s["window"])):
        s["window"] = start["window"]
    if s["efficiency_mode"] not in EFFICIENCY_MODES:
        s["efficiency_mode"] = start["efficiency_mode"]
    for key in ("privacy_mode", "energy_logging"):
        if not isinstance(s[key], bool):
            s[key] = start[key]
    price = parse_number(s["energy_price"])
    s["energy_price"] = price if price is not None and 0 <= price < 100 else start["energy_price"]
    if not isinstance(s["energy_currency"], str) or len(s["energy_currency"].strip()) > 5:
        s["energy_currency"] = start["energy_currency"]
    s["energy_currency"] = s["energy_currency"].strip()
    for key, (low, high) in (("energy_other_watts", OTHER_WATTS), ("energy_psu_efficiency", PSU_EFFICIENCY)):
        if not (isinstance(s[key], int) and not isinstance(s[key], bool) and low <= s[key] <= high):
            s[key] = start[key]
    import fans                                  # here: fans.py needs storage, which needs the path set above
    s["fan_control"] = fans.clean_config(s["fan_control"])
    return s


def parse_number(value):
    """0.35, "0.35" or "0,35" -> 0.35; anything else -> None."""
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    try:
        return float(str(value).strip().replace(",", "."))
    except ValueError:
        return None


def save_settings(settings):
    """Saves the settings. Raises OSError if it doesn't work."""
    storage.save(SETTINGS_FILE, settings)


# ---------- Numbers and dates in the chosen language ----------
def num(value, decimals):
    """Number with the decimal separator of the chosen language: num(2.54, 1) -> "2.5" (German: "2,5")."""
    text = f"{value:.{decimals}f}"
    return text.replace(".", ",") if i18n.LANGUAGE == "de" else text


def short_num(value):
    """Like num(), but without trailing zeros: 0.5 -> "0.5", 2 -> "2" (e.g. for seconds)."""
    text = f"{value:g}"
    return text.replace(".", ",") if i18n.LANGUAGE == "de" else text


def fmt_date(year, month, day):
    """A date in the usual format of the chosen language: German 03.03.2021, otherwise 2021-03-03 (ISO)."""
    return f"{day:02d}.{month:02d}.{year}" if i18n.LANGUAGE == "de" else f"{year}-{month:02d}-{day:02d}"
