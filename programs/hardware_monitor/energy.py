"""
Power consumption of the whole PC: estimate, log, add up.

Most power supplies can't report what they draw (only a few "digital" ones with a USB cable, e.g. Corsair HXi/RMi/AXi
– LibreHardwareMonitor reads those, see IsPsuEnabled). So the consumption at the wall socket is estimated:

    wall power ≈ (CPU package power + graphics card power + other components) ÷ power supply efficiency

CPU and graphics card are measured (they are the parts whose consumption changes a lot); "other components"
(motherboard, RAM, drives, fans, USB devices) is a fixed value from the settings, as is the efficiency. Laptops
running on battery report what they draw in total (discharge rate) – then that measured value is used directly.

Logging: every measurement (icon_interval, 1–10 s) adds watts × seconds to the hour it falls into. Saved in
data\\hardware_monitor_energy.json as {"hours": {"2026-11-06T14": [Wh, measured seconds], …}} – about 300 KB per year.
Only while the monitor runs and logging is switched on; while the PC sleeps nothing is counted (gaps between two
measurements longer than MAX_GAP don't count). Costs are calculated when shown, with the price from the settings.
"""

import ctypes
import json
import logging
import os
import threading
import time
from datetime import datetime, timedelta

from hardware_monitor_settings import num    # first: it adds toolbox\ to the path, where storage.py lives
import storage  # noqa: E402

FILE = storage.settings_file("hardware_monitor_energy")
SAVE_EVERY = 300            # seconds between two saves (and when quitting)
MAX_GAP = 30                # seconds – a longer gap between two measurements (sleep, hang) isn't counted


def uptime_seconds():
    """How long Windows has been running since it started – the same as "Up time" in Task Manager."""
    kernel32 = ctypes.windll.kernel32
    kernel32.GetTickCount64.restype = ctypes.c_uint64
    return kernel32.GetTickCount64() / 1000


def fmt_energy(wh, unit=None):
    """1234 Wh -> "1.23 kWh" (or "1234 Wh" with unit="Wh")."""
    if unit == "Wh" or (unit is None and wh < 1000):
        return f"{num(wh, 0)} Wh"
    kwh = wh / 1000
    return f"{num(kwh, 2 if kwh < 10 else 1 if kwh < 100 else 0)} kWh"


def fmt_duration(seconds):
    """3725 -> "1 h 2 min", 200000 -> "2 d 7 h"."""
    minutes = int(seconds // 60)
    days, hours, minutes = minutes // 1440, minutes // 60 % 24, minutes % 60
    if days:
        return f"{days} d {hours} h"
    return f"{hours} h {minutes} min" if hours else f"{minutes} min"


def fmt_cost(wh, settings):
    """Costs of an amount of energy with the price from the settings, e.g. "0.50 €"."""
    return f"{num(wh / 1000 * settings['energy_price'], 2)} {settings['energy_currency']}".strip()


def wall_watts(cpu, gpu, settings):
    """Estimated power at the wall socket from the measured CPU and GPU power (None = unknown, counts as 0)."""
    dc = (cpu or 0) + (gpu or 0) + settings["energy_other_watts"]
    return dc / (settings["energy_psu_efficiency"] / 100)


class EnergyLog:
    """The logged energy per hour. add() from the measuring thread, the rest from anywhere (with a lock)."""

    def __init__(self, file=None):
        self.file = file or FILE
        self.lock = threading.Lock()
        self.hours = {}                          # "2026-11-06T14" -> [Wh, seconds]
        self.last = None                         # time.monotonic() of the last measurement
        self.saved = time.monotonic()
        self.dirty = False
        self.load()

    def load(self):
        try:
            data = json.loads(self.file.read_text(encoding="utf-8"))
            hours = data.get("hours", {})
            self.hours = {k: [float(v[0]), float(v[1])] for k, v in hours.items()
                          if isinstance(v, list) and len(v) == 2}
        except FileNotFoundError:
            self.hours = {}
        except (OSError, ValueError, TypeError, IndexError):
            logging.exception("%s could not be read – starting empty", self.file)
            self.hours = {}

    def add(self, watts, now=None):
        """One measurement: watts since the last measurement (at most MAX_GAP seconds back)."""
        mono = time.monotonic()
        last, self.last = self.last, mono
        if last is None or watts is None:
            return
        seconds = mono - last
        if seconds <= 0 or seconds > MAX_GAP:
            return
        now = now or datetime.now()
        key = now.strftime("%Y-%m-%dT%H")
        with self.lock:
            entry = self.hours.setdefault(key, [0.0, 0.0])
            entry[0] += watts * seconds / 3600
            entry[1] += seconds
            self.dirty = True
        if mono - self.saved >= SAVE_EVERY:
            self.save()

    def pause(self):
        """Logging switched off: the next measurement starts a new interval."""
        self.last = None

    def save(self):
        """Writes the file (first to a temporary file, then replaces it – a reader never sees half a file)."""
        with self.lock:
            if not self.dirty:
                return
            text = json.dumps({"version": 1, "hours": self.hours}, separators=(",", ":"))
            self.dirty = False
        self.saved = time.monotonic()
        try:
            tmp = self.file.with_suffix(".tmp")
            tmp.write_text(text, encoding="utf-8")
            os.replace(tmp, self.file)
        except OSError:
            logging.exception("%s could not be saved", self.file)

    def delete_all(self):
        """Deletes all logged values – in memory and the file."""
        with self.lock:
            self.hours, self.dirty = {}, False
        self.last = None
        try:
            self.file.unlink(missing_ok=True)
        except OSError:
            logging.exception("%s could not be deleted", self.file)
            return False
        return True

    def snapshot(self):
        with self.lock:
            return {k: list(v) for k, v in self.hours.items()}


# ---------- Adding up (used by the detail window and the Toolbox page) ----------
def buckets(hours, view, now=None):
    """The bars of a view: [(start, end, Wh, measured seconds)], oldest first, including empty ones.
    view: "day" = the last 24 hours, "month" = the last 30 days, "year" = the last 12 months."""
    now = now or datetime.now()
    if view == "day":
        top = now.replace(minute=0, second=0, microsecond=0)
        starts = [top - timedelta(hours=i) for i in range(23, -1, -1)]
        step = lambda s: s + timedelta(hours=1)                                     # noqa: E731
        key = lambda k: datetime.strptime(k, "%Y-%m-%dT%H")                         # noqa: E731
    elif view == "month":
        top = now.replace(hour=0, minute=0, second=0, microsecond=0)
        starts = [top - timedelta(days=i) for i in range(29, -1, -1)]
        step = lambda s: s + timedelta(days=1)                                      # noqa: E731
        key = lambda k: datetime.strptime(k[:10], "%Y-%m-%d")                       # noqa: E731
    else:
        starts = []
        y, m = now.year, now.month
        for _ in range(12):
            starts.insert(0, datetime(y, m, 1))
            y, m = (y, m - 1) if m > 1 else (y - 1, 12)
        step = lambda s: datetime(s.year + (s.month == 12), s.month % 12 + 1, 1)    # noqa: E731
        key = lambda k: datetime(int(k[:4]), int(k[5:7]), 1)                        # noqa: E731
    sums = {}
    for k, (wh, seconds) in hours.items():
        try:
            start = key(k)
        except ValueError:
            continue
        total = sums.setdefault(start, [0.0, 0.0])
        total[0] += wh
        total[1] += seconds
    return [(s, step(s), *sums.get(s, (0.0, 0.0))) for s in starts]


def totals(hours, now=None):
    """{"today": (Wh, seconds), "month": …, "year": …, "all": …, "since": first day or None}."""
    now = now or datetime.now()
    today, month, year = now.strftime("%Y-%m-%d"), now.strftime("%Y-%m"), now.strftime("%Y")
    result = {name: [0.0, 0.0] for name in ("today", "month", "year", "all")}
    for k, (wh, seconds) in hours.items():
        for name, match in (("today", k.startswith(today)), ("month", k.startswith(month)),
                            ("year", k.startswith(year)), ("all", True)):
            if match:
                result[name][0] += wh
                result[name][1] += seconds
    result = {name: tuple(v) for name, v in result.items()}
    result["since"] = min(hours)[:10] if hours else None
    return result
