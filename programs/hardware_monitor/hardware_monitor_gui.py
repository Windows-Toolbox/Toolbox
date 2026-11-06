"""
Page of the Hardware Monitor in the Toolbox GUI (building blocks from toolbox/base.py).

The Toolbox runs without admin rights, hardware_monitor with them. That's why:
  - status, quitting and "reload settings" go via a local connection (IPC_PORT),
  - starting goes via the scheduled task (no UAC prompt) or, if need be, directly with a UAC prompt,
  - the Toolbox writes settings to data\\hardware_monitor.json and then tells hardware_monitor "reload".
All texts are in i18n.json – only their ID is here: t("page.…") (see toolbox\\i18n.py).
"""

import json
import socket
import sys
import time
from pathlib import Path
from tkinter import messagebox

import winutil
from base import (BAD, GOOD, MUTED, WARN, Button, Buttons, Choice, Combo, Entry, Program, Section, Slider, Switch,
                  Text, ToolPage)

FOLDER = Path(__file__).resolve().parent
sys.path.append(str(FOLDER))
# Defaults, rates, port, task, loading/saving, texts – shared with hardware_monitor.py
import hardware_monitor_settings as hm  # noqa: E402
from hardware_monitor_settings import num, short_num, t  # noqa: E402
import energy  # noqa: E402  (power consumption: file and adding up – the logging itself runs in the monitor)

# Created by Setup.exe: the starter HardwareMonitor.exe (finds runtime\ relative to itself, hence without
# arguments) and behind it pythonw.exe with its own name and icon – that way the process is called
# "Hardware Monitor" in Task Manager instead of "Python" (setup\make_exe.py)
STARTER = FOLDER / hm.MANIFEST["starter"]

# Sensor names of LibreHardwareMonitor – they stay English; in the list "Automatic" comes before them
CPU_CHOICES = ["Core (Tctl/Tdie)", "CPU Package", "Core Max", "Core Average", "CCD1 (Tdie)", "CCD2 (Tdie)"]
GPU_CHOICES = ["GPU Core", "GPU Hot Spot", "GPU Memory Junction"]


def seconds_choices(values):
    """(0.5, 1, 2) -> {"0.5 s": 0.5, "1 s": 1, "2 s": 2} for the choice buttons."""
    return {f"{short_num(v)} s": v for v in values}


def ipc(command, timeout=0.6):
    """Sends a command to hardware_monitor. Answer as a dict – or None if it isn't running."""
    try:
        with socket.create_connection(("127.0.0.1", hm.IPC_PORT), timeout=timeout) as conn:
            conn.sendall(command.encode("utf-8") + b"\n")
            data = b""
            while not data.endswith(b"\n"):
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data += chunk
        return json.loads(data) if data else None
    except (OSError, ValueError):
        return None


def fmt_temp(value):
    return "–" if value is None else f"{value:.0f} °C"


def same_path(a, b):
    try:
        return Path(a).resolve() == Path(b).resolve()
    except (OSError, ValueError):
        return False


def task_points_here(info):
    """Does the task really start this monitor – the starter without arguments – and not e.g. the place where the
    folder was before it was moved? (setup.ps1 checks the same in step 6.)"""
    return same_path(info.get("execute") or "", STARTER) and not (info.get("arguments") or "").strip()


# ---------- Controlling the program (also for "Toolbox and all programs" in the Toolbox settings) ----------
# All of them block for a while – only call them in the background (app.run_async).
def is_running():
    return ipc("status") is not None


def stop_program():
    """Asks the program to quit and waits for the process to end (otherwise a direct restart fails)."""
    status = ipc("status")
    if status is None:
        return True
    ipc("quit")
    if winutil.wait_for_exit(status.get("pid"), timeout=10):
        return True
    for _ in range(50):                          # fallback: wait until it no longer answers (up to 5 s)
        if ipc("status", timeout=0.2) is None:
            return True
        time.sleep(0.1)
    return False


def start_program():
    """Via the scheduled task (without a prompt) – or, if there's no matching one, directly with a UAC prompt."""
    info = winutil.task_info(hm.TASK)
    if info and info.get("state") != "Disabled" and task_points_here(info) and winutil.run_task(hm.TASK):
        return True
    return winutil.run_elevated(str(STARTER), cwd=str(FOLDER))


def reload_program():
    """Let it read settings and language again (e.g. after a language switch in the Toolbox)."""
    ipc("reload")


class HardwareMonitorPage(ToolPage):
    settings_file = hm.SETTINGS_FILE
    defaults = hm.DEFAULT_SETTINGS
    visible = False
    poll_job = None
    running_order = None         # icon order with which the running program was started
    changed = frozenset()        # keys changed on this page since the last save (see save())

    def layout(self):
        grey = {"width": 150, "style": "gray"}
        auto = {"auto": t("page.automatic")}
        return [
            Section(t("page.status"),
                    Text(f"●  {t('page.status.checking')}", name="status", size=14),
                    Buttons(Button(t("page.start"), self.start, name="start"),
                            Button(t("page.stop"), self.stop, name="stop"),
                            Button(t("page.restart"), self.restart, name="restart")),
                    Text(f"⚠  {t('page.starter_missing', starter=STARTER)}", name="missing", color=WARN,
                         hidden=True)),
            Section(t("page.autostart"),
                    Text(t("page.autostart.text")),
                    Switch(t("page.autostart.switch"), command=self.toggle_autostart, name="autostart"),
                    Text(name="task_label"),
                    Buttons(Button(t("page.task.set_up"), self.setup_task, name="task_button", width=170))),
            Section(t("page.refresh"),
                    Text(t("page.refresh.text")),
                    Choice(t("page.refresh.icons"), "icon_interval", seconds_choices(hm.ICON_RATES)),
                    Choice(t("page.refresh.window"), "popup_interval", seconds_choices(hm.POPUP_RATES)),
                    Choice(t("page.efficiency"), "efficiency_mode",
                           dict(zip((t("page.automatic"), t("page.efficiency.on"), t("page.efficiency.off")),
                                    hm.EFFICIENCY_MODES)),
                           info=t("page.efficiency.info"))),
            Section(t("page.display"),
                    Choice(t("page.order"), "icon_order",
                           {t("page.order.cpu"): ["cpu", "gpu"], t("page.order.gpu"): ["gpu", "cpu"]}),
                    Text(t("page.order.hint"), name="order_hint", color=WARN, hidden=True),
                    Combo(t("page.sensor_cpu"), "cpu_sensor", [auto["auto"], *CPU_CHOICES], names=auto),
                    Combo(t("page.sensor_gpu"), "gpu_sensor", [auto["auto"], *GPU_CHOICES], names=auto),
                    Text(t("page.sensor.text")),
                    Text(t("page.colors.text")),
                    Slider(t("page.cpu_yellow"), "cpu_warn_temp", 40, 100, color=WARN),
                    Slider(t("page.cpu_red"), "cpu_hot_temp", 41, 110, color=BAD),
                    Slider(t("page.gpu_yellow"), "gpu_warn_temp", 40, 100, color=WARN),
                    Slider(t("page.gpu_red"), "gpu_hot_temp", 41, 110, color=BAD)),
            Section(t("page.privacy"),
                    Text(t("page.privacy.text")),
                    Switch(t("page.privacy.switch"), key="privacy_mode")),
            Section(t("page.power"),
                    Text(t("page.power.text")),
                    Switch(t("page.power.switch"), key="energy_logging"),
                    Text(f"●  {t('page.status.checking')}", name="power_summary"),
                    Entry(t("page.power.price"), "energy_price", width=90),
                    Entry(t("page.power.currency"), "energy_currency", width=60),
                    Slider(t("page.power.other"), "energy_other_watts", *hm.OTHER_WATTS),
                    Slider(t("page.power.efficiency"), "energy_psu_efficiency", *hm.PSU_EFFICIENCY),
                    Text(t("page.power.calibrate")),
                    Buttons(Button(t("page.power.show"), self.show_power, width=150, style="gray"),
                            Button(t("page.power.delete"), self.delete_power, width=170, style="danger"))),
            Section(t("page.fans"),
                    Text(t("page.fans.text")),
                    Buttons(Button(t("page.fans.show"), lambda: self.show_window("show_fans"), width=170,
                                   style="gray"))),
            Section(t("page.detail_window"),
                    Text(t("page.detail_window.text")),
                    Buttons(Button(t("page.open_folder"), lambda: self.open(FOLDER), width=150, style="gray",
                                   target=FOLDER))),
            Section(t("page.advanced"),
                    Buttons(Button(t("page.open_log"), lambda: self.open(hm.LOG_FILE), target=hm.LOG_FILE, **grey),
                            Button(t("page.open_settings_file"), lambda: self.open(self.settings_file),
                                   target=self.settings_file, **grey),
                            Button(t("page.reset"), self.reset, width=150, style="danger"))),
        ]

    # ---------- Settings ----------
    def load(self):
        return hm.load_settings()                # with the same check as in the monitor itself

    def save(self):
        """Only writes what was changed on this page. The monitor changes values itself too (rates and privacy mode
        in the right-click menu, window position) – an older state shown here must not overwrite them."""
        fresh = self.load()
        fresh.update({key: self.data[key] for key in self.changed})
        self.data, self.changed = fresh, frozenset()
        self.refresh()
        super().save()

    def reset(self):
        self.changed = frozenset(self.defaults)
        super().reset()

    def set(self, key, value, later=False):
        """Typed price -> number ("0,35" works too); invalid or unchanged input isn't saved."""
        if key == "energy_price":
            price = hm.parse_number(value)
            if price is None or not 0 <= price < 100:
                self.refresh(key)                        # show the valid value again
                self.app.toast(t("page.power.price_invalid"), error=True)
                return
            value = round(price, 4)
        elif key == "energy_currency":
            value = str(value).strip()[:5]
        if key in ("energy_price", "energy_currency") and value == self.data.get(key):
            self.refresh(key)                            # e.g. only left the field
            return
        super().set(key, value, later)

    def on_change(self, key, value):
        self.changed |= {key}
        # yellow always has to be below red – drag the other slider along if necessary
        kind, _, limit = key.partition("_")              # e.g. "cpu", "warn_temp"
        warn, hot = f"{kind}_warn_temp", f"{kind}_hot_temp"
        if limit == "warn_temp" and value >= self.data[hot]:
            self.data[hot] = value + 1
            self.changed |= {hot}
            self.refresh(hot)
        elif limit == "hot_temp" and value <= self.data[warn]:
            self.data[warn] = value - 1
            self.changed |= {warn}
            self.refresh(warn)
        elif key == "icon_order":
            self.update_order_hint()

    def on_saved(self):
        self.app.run_async(reload_program)

    def update_order_hint(self):
        self.show("order_hint", self.running_order is not None and self.running_order != self.data["icon_order"])

    # ---------- Status and control ----------
    def on_show(self):
        if self.visible:
            return
        self.visible = True
        super().on_show()                        # e.g. take over rates changed via the right-click menu
        self.poll_status()
        self.refresh_task()

    def on_hide(self):
        self.visible = False
        if self.poll_job:
            self.after_cancel(self.poll_job)
            self.poll_job = None

    def poll_status(self):
        if not self.visible:
            return
        self.app.run_async(lambda: ipc("status"), self.show_status)
        self.poll_job = self.after(2000, self.poll_status)

    def poll_status_soon(self):
        """After start/stop look a few times quickly instead of waiting 2 seconds."""
        for delay in (400, 1200, 2500):
            self.after(delay, lambda: self.app.run_async(lambda: ipc("status"), self.show_status))

    def show_status(self, status):
        if not self.winfo_exists():
            return                               # the page has been rebuilt in the meantime (language switch)
        running = bool(status and status.get("ok"))
        self.running_order = status.get("order") if running else None
        if running:                              # only the icons that exist (without GPU temperature no GPU icon)
            values = [f"{kind.upper()} {fmt_temp(status.get(kind))}" for kind in status.get("icons", ["cpu", "gpu"])]
            text = "  ·  ".join([f"●  {t('page.status.running')}"] + values)
        else:
            text = f"●  {t('page.status.stopped')}"
        self.w["status"].configure(text=text, text_color=GOOD if running else MUTED)
        self.enable("start", not running)
        self.enable("stop", running)
        self.enable("restart", running)
        self.show("missing", not STARTER.exists())
        self.update_order_hint()
        privacy = status.get("privacy_mode") if running else None
        if privacy is not None and privacy != self.data["privacy_mode"] and "privacy_mode" not in self.changed:
            self.data["privacy_mode"] = privacy      # switched in the detail window or right-click menu
            self.refresh("privacy_mode")
        self.show_power_summary(status if running else None)

    # ---------- Power consumption ----------
    def show_power_summary(self, status):
        """One line under the switch: now, today, this month, in total. From the running monitor (it has the values
        of the last minutes, which are only saved every 5 minutes) – otherwise from the file."""
        if status and status.get("energy"):
            totals, watts = status["energy"], status.get("power")
        else:
            totals, watts = self.file_totals(), None
        parts = [t("page.power.now", watts=num(watts, 0))] if watts is not None else []
        for name in ("today", "month", "all"):
            wh = totals[name][0]
            if wh:
                parts.append(t(f"page.power.{name}", energy=energy.fmt_energy(wh, "kWh"),
                               cost=energy.fmt_cost(wh, self.data)))
        if totals.get("since"):
            parts.append(t("page.power.since", since=totals["since"]))
        logging_on = self.data["energy_logging"]
        if not parts:
            text = t("page.power.nothing_yet" if logging_on else "page.power.off")
        else:
            text = "  ·  ".join(parts)
        self.w["power_summary"].configure(text=f"●  {text}", text_color=GOOD if logging_on else MUTED)

    def file_totals(self):
        """Totals from data\\hardware_monitor_energy.json – read again only when the file has changed."""
        try:
            mtime = energy.FILE.stat().st_mtime
        except OSError:
            mtime = None
        if getattr(self, "_file_totals", (None, None))[0] != mtime or mtime is None:
            self._file_totals = (mtime, energy.totals(energy.EnergyLog().snapshot()))
        return self._file_totals[1]

    def show_power(self):
        self.show_window("show_power")

    def show_window(self, command):
        """Open the detail window on the tab Power ("show_power") or Fans ("show_fans") – only while it runs."""
        def done(reply):
            if not reply:
                self.app.toast(t("page.power.not_running"), error=True)

        self.app.run_async(lambda: ipc(command), done)

    def delete_power(self):
        if not messagebox.askyesno(t("page.power.delete.title"), t("page.power.delete.question"), parent=self):
            return

        def delete():
            if ipc("status") is not None:            # the monitor has the values in memory too – it deletes both
                reply = ipc("energy_delete")
                return bool(reply and reply.get("ok"))
            try:
                energy.FILE.unlink(missing_ok=True)
                return True
            except OSError:
                return False

        def done(ok):
            self.app.toast(t("page.power.deleted") if ok else t("page.power.delete_failed"), error=not ok)
            if self.winfo_exists():
                self.show_power_summary(None)

        self.app.run_async(delete, done)

    def say(self, text):
        self.w["status"].configure(text=f"●  {text}", text_color=MUTED)

    def start(self):
        if not STARTER.exists():
            self.app.toast(t("page.toast.starter_missing", starter=STARTER.name), error=True)
            return
        self.say(t("page.status.starting"))
        self.enable("start", False)

        def done(ok):
            if not ok:
                self.app.toast(t("page.toast.start_failed"), error=True)
            self.poll_status_soon()

        self.app.run_async(start_program, done)

    def stop(self):
        self.say(t("page.status.stopping"))
        self.app.run_async(lambda: ipc("quit"), lambda result: self.poll_status_soon())

    def restart(self):
        self.say(t("page.status.restarting"))

        def done(stopped):
            if stopped:
                self.start()
            else:
                self.app.toast(t("page.toast.not_responding"), error=True)

        self.app.run_async(stop_program, done)

    # ---------- Autostart ----------
    def refresh_task(self):
        task = hm.TASK
        self.w["autostart"].configure(state="disabled")
        self.w["task_label"].configure(text=t("page.task.checking", task=task), text_color=MUTED)
        self.app.run_async(lambda: winutil.task_info(task), self.show_task)

    def show_task(self, info):
        if not self.winfo_exists():
            return                               # the page has been rebuilt in the meantime (language switch)
        task, switch, label, button = hm.TASK, self.w["autostart"], self.w["task_label"], self.w["task_button"]
        if info is None:
            switch.deselect()
            button.configure(text=t("page.task.set_up"))
            label.configure(text=t("page.task.missing", task=task), text_color=WARN)
            return
        enabled = info.get("state") != "Disabled"
        switch.configure(state="normal")
        switch.select() if enabled else switch.deselect()
        if task_points_here(info):
            button.configure(text=t("page.task.set_up_again"))
            label.configure(text=t("page.task.enabled" if enabled else "page.task.disabled", task=task),
                            text_color=MUTED)
        else:
            button.configure(text=t("page.task.update"))
            text = t("page.task.other_location", task=task, target=info.get("execute"),
                     arguments=info.get("arguments"))
            label.configure(text=f"⚠  {text}", text_color=WARN)

    def setup_task(self):
        if not STARTER.exists():
            self.app.toast(t("page.toast.starter_missing", starter=STARTER.name), error=True)
            return
        self.w["task_button"].configure(state="disabled")
        self.w["task_label"].configure(text=t("page.task.setting_up"), text_color=MUTED)

        def done(ok):
            if not self.winfo_exists():
                return
            self.w["task_button"].configure(state="normal")
            self.app.toast(t("page.toast.task_set_up") if ok else t("page.toast.setup_failed"), error=not ok)
            self.refresh_task()

        self.app.run_async(lambda: winutil.register_task(hm.TASK, STARTER, cwd=FOLDER), done)

    def toggle_autostart(self):
        enabled = bool(self.w["autostart"].get())
        self.w["autostart"].configure(state="disabled")

        def done(ok):
            if not ok:
                self.app.toast(t("page.toast.change_failed"), error=True)
            if self.winfo_exists():
                self.refresh_task()

        self.app.run_async(lambda: winutil.set_task_enabled(hm.TASK, enabled), done)


PAGE = HardwareMonitorPage
PROGRAM = Program(is_running, stop_program, start_program, reload_program)   # for "Toolbox and all programs" and the language switch
