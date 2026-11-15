"""
Page for small programs that are only a script – they don't need a page file of their own.

Such a program folder holds just program.json, the script and (optionally) i18n.json. Instead of "page" the
manifest names "run":

    {
      "id": "rayman_legends", "name": "Rayman Legends", "version": "1.0", "category": "Games",
      "description": "…",
      "run": "rayman_borderless.ps1",                  the script next to it: .ps1, .cmd/.bat, .py or .exe
      "launch": "uplay://launch/410/0",                optional: also opened by the start button (game, website …)
      "options": {"Pos": {"values": ["Left", "Center", "Right"], "default": "Center"}}
                                                       optional: choices on the page, passed on as -Pos Center
                                                       (a plain list works too – then the first value is the default)
    }

The start button runs the script without a window (no console pops up) and opens "launch" afterwards. The page shows
whether the script is still running and can stop it. The chosen options are in data\\<program>.json as usual – a
script that should react to a change while it runs reads that file again itself (example: rayman_borderless.ps1).

Texts from the program's i18n.json – all optional, without them the page uses the Toolbox's own:
  program.name, program.description     like for every program
  page.hint                              text above the start button (e.g. what has to be set in the game first)
  page.button                            label of the start button (default: "Start")
  option.<name>                          label of an option, option.<name>.<value> the label of one of its values
"""

import logging
import os
import subprocess
from pathlib import Path

from base import BAD, GOOD, MUTED, Button, Buttons, Choice, Section, Text, ToolPage, t

ROOT = Path(__file__).resolve().parents[1]
PYTHONW = ROOT / "runtime" / "pythonw.exe"      # the Toolbox's own Python (runtime\Toolbox.exe is a renamed copy)
NO_WINDOW = subprocess.CREATE_NO_WINDOW

# Running scripts per program ID – here and not on the page, because pages are rebuilt (language change)
RUNNING = {}


def command(script):
    """How a script is started, by its file type."""
    kind = script.suffix.lower()
    if kind == ".ps1":
        return ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(script)]
    if kind in (".cmd", ".bat"):
        return ["cmd.exe", "/c", str(script)]
    if kind == ".py":
        return [str(PYTHONW), str(script)]
    return [str(script)]                                   # .exe


class ScriptPage(ToolPage):
    manifest = {}                                          # set by page_for()

    @property
    def texts(self):
        return self.manifest.get("texts")

    def text(self, id, default):
        """Text from the program's i18n.json, otherwise default."""
        return self.texts(id) if self.texts and id in self.texts else default

    def layout(self):
        options = [Choice(self.text(f"option.{name}", name), name,
                          {self.text(f"option.{name}.{value}", value): value for value in option_values(spec)})
                   for name, spec in self.options().items()]
        hint = self.text("page.hint", "")
        return [
            Section(t("script.section"),
                    *([Text(hint, color=MUTED)] if hint else []),
                    *options,
                    Text(name="status", color=MUTED),
                    Buttons(Button(self.text("page.button", t("script.start")), self.start, width=150,
                                   target=self.manifest.get("launch") or self.script()),
                            Button(t("script.stop"), self.stop, name="stop", width=110, style="gray"),
                            Button(t("script.open_folder"), lambda: self.open(self.manifest["folder"]), width=150,
                                   style="gray", target=self.manifest["folder"]))),
        ]

    def options(self):
        return self.manifest.get("options") or {}

    def script(self):
        return self.manifest["folder"] / self.manifest["run"]

    # --- Start and stop ---
    def running(self):
        process = RUNNING.get(self.info.id)
        return process is not None and process.poll() is None

    def start(self):
        if not self.running():
            arguments = []
            for name, spec in self.options().items():
                arguments += [f"-{name}", str(self.data.get(name, option_default(spec)))]
            try:
                RUNNING[self.info.id] = subprocess.Popen(command(self.script()) + arguments,
                                                         cwd=str(self.manifest["folder"]), creationflags=NO_WINDOW)
            except OSError:
                logging.exception("%s could not be started", self.script())
                self.app.toast(t("toast.did_not_start", name=self.script().name), error=True)
                return
        launch = self.manifest.get("launch")
        if launch:
            try:
                os.startfile(launch)
            except OSError:
                logging.exception("%s could not be opened", launch)
                self.app.toast(t("toast.did_not_start", name=launch), error=True)
        self.show_status()

    def stop(self):
        process = RUNNING.get(self.info.id)
        if process and process.poll() is None:
            # The whole tree – a .cmd may have started further programs
            subprocess.run(["taskkill", "/T", "/F", "/PID", str(process.pid)], capture_output=True,
                           creationflags=NO_WINDOW)
        self.show_status()

    # --- Status line: checked every second while the page is visible ---
    def show_status(self):
        running = self.running()
        self.w["status"].configure(text=t("script.running" if running else "script.not_running"),
                                   text_color=GOOD if running else MUTED)
        self.enable("stop", running)
        if not self.script().exists():
            self.w["status"].configure(text=t("toast.not_found", name=self.script().name), text_color=BAD)

    def on_show(self):
        super().on_show()
        self.poll()

    def poll(self):
        self.show_status()
        self.poll_job = self.after(1000, self.poll)

    def on_hide(self):
        if getattr(self, "poll_job", None):
            self.after_cancel(self.poll_job)
            self.poll_job = None


def option_values(spec):
    """An option is a list of values (the first is the default) or {"values": [...], "default": value}."""
    return spec["values"] if isinstance(spec, dict) else spec


def option_default(spec):
    return spec.get("default", spec["values"][0]) if isinstance(spec, dict) else spec[0]


def page_for(manifest):
    """Its own page class per program, with the manifest attached and the default of each option."""
    defaults = {name: option_default(spec) for name, spec in (manifest.get("options") or {}).items()}
    return type(f"ScriptPage_{manifest['id']}", (ScriptPage,), {"manifest": manifest, "defaults": defaults})
