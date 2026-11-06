"""
Fan control: curves (temperature -> %) for the graphics card fans and the motherboard fan headers – like the fan curve
in MSI Afterburner. Uses LibreHardwareMonitor's IControl (sensor.Control): SetSoftware(%) takes a fan over,
SetDefault() hands it back to the graphics card or the BIOS.

One item per graphics card (all its fans follow one curve, like in Afterburner) and one per motherboard fan header.
Per item in the settings ("fan_control"): mode "auto" (not controlled – the default), "curve" or "fixed", the source
temperature ("gpu", "cpu" or "max" = the higher one) and the curve points [[°C, %], …] or the fixed %.

Safety:
  - Only items switched to "curve" or "fixed" are touched; everything else stays as without this program.
  - At or above the red limit of the traffic light (gpu_hot_temp / cpu_hot_temp) a controlled fan runs at 100 %.
  - No temperature (sensor gone) or an error -> the fan is handed back to the card/BIOS.
  - When quitting all fans are handed back (also after an error in the measuring thread).
  - If the monitor is killed (Task Manager, crash), the last speed stays until Windows restarts – card and CPU then
    protect themselves by throttling. A marker file notices that on the next start (shown in the tab "Fans").
  - Hysteresis: a fan only slows down again once the temperature has dropped HYSTERESIS °C below the value at which
    it was last sped up – otherwise it would hum up and down with every degree.
"""

import logging
from dataclasses import dataclass, field

import storage

INTERVAL = 2             # seconds between two adjustments while a fan is controlled (also measures the icons that often)
HYSTERESIS = 3           # °C
REAPPLY = 30             # seconds – set the speed again anyway (after sleep the driver may have taken over again)
MODES = ("auto", "curve", "fixed")
SOURCES = ("gpu", "cpu", "max")
MARKER = storage.DATA_DIR / "hardware_monitor_fans.active"    # exists while fans are controlled
# Starting curves – quieter than most factory curves, 100 % before it gets critical
DEFAULT_CURVES = {
    "gpu": [[40, 30], [60, 40], [70, 55], [78, 75], [83, 100]],
    "case": [[40, 25], [60, 35], [70, 50], [80, 75], [88, 100]],
}
DEFAULT_FIXED = 50
MAX_POINTS = 8


@dataclass
class FanItem:
    """A fan (or all fans of a graphics card) that can be controlled."""
    id: str                   # LHM identifier: of the graphics card or of the motherboard control sensor
    kind: str                 # "gpu" or "case"
    name: str                 # e.g. "NVIDIA GeForce RTX 4060 Ti" or "Fan #2"
    controls: list            # .NET IControl objects
    control_ids: list = field(default_factory=list)      # sensor IDs of the controls (current % for the window)
    fan_ids: list = field(default_factory=list)          # sensor IDs of the speeds (RPM for the window)
    minimum: float = 0
    maximum: float = 100


def curve_value(points, temp):
    """% of the curve at temp: linear between the points, flat before the first and after the last one."""
    points = sorted(points)
    if temp <= points[0][0]:
        return points[0][1]
    for (t0, p0), (t1, p1) in zip(points, points[1:]):
        if temp <= t1:
            return p1 if t1 == t0 else p0 + (p1 - p0) * (temp - t0) / (t1 - t0)
    return points[-1][1]


def default_config(kind):
    return {"mode": "auto", "source": "gpu" if kind == "gpu" else "cpu",
            "curve": [list(p) for p in DEFAULT_CURVES[kind]], "fixed": DEFAULT_FIXED}


def clean_config(config):
    """Settings "fan_control" checked: invalid entries are dropped or get defaults (called by load_settings)."""
    result = {}
    if not isinstance(config, dict):
        return result
    for fan_id, entry in config.items():
        if not isinstance(fan_id, str) or not isinstance(entry, dict):
            continue
        clean = {"mode": entry.get("mode") if entry.get("mode") in MODES else "auto"}
        if entry.get("source") in SOURCES:               # missing: default_config() decides (GPU fans: "gpu")
            clean["source"] = entry["source"]
        curve = entry.get("curve")
        if (isinstance(curve, list) and 2 <= len(curve) <= MAX_POINTS
                and all(isinstance(p, list) and len(p) == 2 and all(isinstance(v, int) and not isinstance(v, bool)
                                                                     for v in p)
                        and 0 <= p[0] <= 110 and 0 <= p[1] <= 100 for p in curve)):
            clean["curve"] = sorted(curve)
        fixed = entry.get("fixed")
        if isinstance(fixed, int) and not isinstance(fixed, bool) and 0 <= fixed <= 100:
            clean["fixed"] = fixed
        result[fan_id] = clean
    return result


class FanController:
    """Lives in the measuring thread – only it talks to LibreHardwareMonitor. The window reads `info` and
    `notice` (plain Python data, replaced as a whole)."""

    def __init__(self):
        self.items = []
        self.state = {}               # id -> {"software": bool, "pct": %, "temp": °C when set, "time": s, "note": str}
        self.info = []                # for the window: one dict per item
        self.notice = None            # "crashed" if the marker file was left behind by the last run
        if MARKER.exists():
            logging.warning("The last run ended while it controlled fans – they kept their last speed until now")
            self.notice = "crashed"
            self.remove_marker()

    def set_items(self, items):
        self.items = items
        logging.info("Controllable fans: %s", ", ".join(f"{i.name} ({i.id})" for i in items) or "none")
        self.update_info()

    def wanted(self, config):
        """True if any fan should be controlled (or still is) – then the measuring thread adjusts every INTERVAL s."""
        return any(s["software"] for s in self.state.values()) or any(
            (config.get(item.id) or {}).get("mode", "auto") != "auto" for item in self.items)

    def apply(self, config, temps, settings, now):
        """Adjusts all fans. config: settings["fan_control"]; temps: {"cpu": °C, "gpu": °C} (None = unknown)."""
        for item in self.items:
            entry = {**default_config(item.kind), **(config.get(item.id) or {})}
            state = self.state.setdefault(item.id, {"software": False, "pct": None, "temp": None, "time": 0,
                                                     "note": None})
            if entry["mode"] == "auto":
                if state["software"]:
                    self.release(item)
                state["note"] = None
                continue
            temp, hot = self.source_temp(entry["source"], temps, settings)
            if temp is None:
                if state["software"]:
                    self.release(item)
                state["note"] = "no_temperature"
                continue
            if hot:
                target, state["note"] = 100, "hot"
            else:
                target = curve_value(entry["curve"], temp) if entry["mode"] == "curve" else entry["fixed"]
                state["note"] = None
            target = round(min(max(target, item.minimum), item.maximum))
            last = state["pct"]
            if last is not None and target < last and temp > state["temp"] - HYSTERESIS:
                target = last                    # hysteresis: not slower yet
            changed = target != last
            if changed or not state["software"] or now - state["time"] >= REAPPLY:
                # remembered: the temperature at which this speed was chosen (reference for the hysteresis)
                self.set_speed(item, target, temp if changed or state["temp"] is None else state["temp"], now)
        self.update_marker()
        self.update_info()

    @staticmethod
    def source_temp(source, temps, settings):
        """(temperature, at or above the red limit?) for "gpu", "cpu" or "max"."""
        values = {kind: temps.get(kind) for kind in ("cpu", "gpu")}
        hot = {kind: v is not None and v >= settings[f"{kind}_hot_temp"] for kind, v in values.items()}
        if source == "max":
            known = [v for v in values.values() if v is not None]
            return (max(known) if known else None), hot["cpu"] or hot["gpu"]
        return values.get(source), hot.get(source, False)

    def set_speed(self, item, pct, temp, now):
        state = self.state[item.id]
        try:
            for control in item.controls:
                control.SetSoftware(float(pct))
        except Exception:
            logging.exception("Fan %s could not be set to %s %%", item.name, pct)
            self.release(item)
            state["note"] = "error"
            return
        if not state["software"]:
            logging.info("Fan control: %s taken over (%s %% at %s °C)", item.name, pct, temp)
        state.update(software=True, pct=pct, temp=temp, time=now)

    def release(self, item):
        """Hand the fan back to the graphics card or the BIOS."""
        state = self.state.setdefault(item.id, {})
        try:
            for control in item.controls:
                control.SetDefault()
            if state.get("software"):
                logging.info("Fan control: %s handed back", item.name)
        except Exception:
            logging.exception("Fan %s could not be handed back", item.name)
        state.update(software=False, pct=None, temp=None, time=0)

    def release_all(self):
        """When quitting: hand every controlled fan back."""
        for item in self.items:
            if self.state.get(item.id, {}).get("software"):
                self.release(item)
        self.update_marker()
        self.update_info()

    def update_marker(self):
        if any(s.get("software") for s in self.state.values()):
            if not MARKER.exists():
                try:
                    MARKER.write_text("Fans are controlled by the Hardware Monitor right now.\n", encoding="utf-8")
                except OSError:
                    logging.exception("%s could not be written", MARKER)
        else:
            self.remove_marker()

    @staticmethod
    def remove_marker():
        try:
            MARKER.unlink(missing_ok=True)
        except OSError:
            logging.exception("%s could not be deleted", MARKER)

    def update_info(self):
        self.info = [{"id": item.id, "kind": item.kind, "name": item.name, "control_ids": list(item.control_ids),
                      "fan_ids": list(item.fan_ids), "minimum": item.minimum, "maximum": item.maximum,
                      **{k: self.state.get(item.id, {}).get(k) for k in ("software", "pct", "note")}}
                     for item in self.items]
