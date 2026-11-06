"""
The tab "Fans" in the detail window: the fans that can be controlled, and for the selected one its mode (Auto /
Curve / Fixed), the source temperature and a curve editor like in MSI Afterburner.

Curve editor (tk.Canvas): x = temperature 20–100 °C, y = fan speed 0–100 %. Drag a point to move it (it stays
between its neighbors), double-click on an empty spot to add one (at most fans.MAX_POINTS), right-click a point to
remove it (at least 2 stay). A vertical line marks the current temperature with the speed the fan gets right now;
from the red limit of the traffic light on the fan always runs at 100 % (shaded red).
Changes are saved and applied right away – the measuring thread does the actual controlling (fans.py).
"""

import tkinter as tk
from tkinter import ttk

import fans
from hardware_monitor_settings import num, t

LINE = "#25a774"         # curve and points (same green as the bars in the tab Power)
ACTIVE = "#3ddc97"       # point under the mouse / being dragged
GRID = "#213443"
SUB, FG, SURFACE, RED = "#8fa3b8", "#e2e8f0", "#152736", "#ff6464"
T_MIN, T_MAX = 20, 100   # temperature axis of the editor


class FanView:
    """window: the DetailWindow (px, footer, app, snapshot). master is the frame in the notebook."""

    def __init__(self, frame, window):
        self.window, self.app, self.master = window, window.app, frame
        self.selected = None                     # id of the fan shown in the editor
        self.drag = None                         # index of the point being dragged
        self.hot_point = None                    # index of the point under the mouse
        self.points = []                         # curve being edited: [[°C, %], …]
        self.layout = None                       # (left, top, width, height) of the plot
        px = window.px

        self.note = ttk.Label(frame, style="Foot.TLabel", justify="left")
        self.note.pack(side="top", fill="x", pady=(px(6), px(4)))
        self.note.bind("<Configure>", lambda e: self.note.configure(wraplength=e.width))

        self.tree = ttk.Treeview(frame, columns=("speed", "control", "mode"), show="tree", selectmode="browse",
                                 height=4)
        self.tree.column("#0", width=px(200), stretch=True)
        self.tree.column("speed", width=px(110), stretch=False, anchor="e")      # "1210 / 1195 RPM" (two fans)
        self.tree.column("control", width=px(50), stretch=False, anchor="e")
        self.tree.column("mode", width=px(140), stretch=True, anchor="w")
        self.tree.tag_configure("hint", foreground=SUB)
        self.tree.tag_configure("controlled", foreground=ACTIVE)
        self.tree.pack(side="top", fill="x")
        self.tree.bind("<<TreeviewSelect>>", lambda e: self.select(self.tree.selection()[0]
                                                                    if self.tree.selection() else None))
        self.tree.bind("<Motion>", lambda e: window.footer(t("fans.list.help")))

        editor = self.editor = ttk.Frame(frame)
        editor.pack(side="top", fill="both", expand=True, pady=(px(8), 0))
        row = ttk.Frame(editor)
        row.pack(side="top", fill="x")
        self.mode = tk.StringVar(frame, "auto")
        ttk.Label(row, text=t("fans.mode"), style="Foot.TLabel").pack(side="left", padx=(0, px(6)))
        for mode in fans.MODES:
            button = ttk.Radiobutton(row, text=t(f"fans.mode.{mode}"), value=mode, variable=self.mode,
                                     style="SubTab.Toolbutton", cursor="hand2", command=self.mode_changed)
            button.pack(side="left")
            button.bind("<Enter>", lambda e, m=mode: window.footer(t(f"fans.mode.{m}.help")))
        self.source = tk.StringVar(frame, "gpu")
        self.source_buttons = []
        for source in reversed(fans.SOURCES):
            button = ttk.Radiobutton(row, text=t(f"fans.source.{source}"), value=source, variable=self.source,
                                     style="SubTab.Toolbutton", cursor="hand2", command=self.changed)
            button.pack(side="right")
            button.bind("<Enter>", lambda e: window.footer(t("fans.source.help")))
            self.source_buttons.append(button)
        self.source_label = ttk.Label(row, text=t("fans.source"), style="Foot.TLabel")
        self.source_label.pack(side="right", padx=(0, px(6)))

        self.fixed_row = ttk.Frame(editor)
        self.fixed = tk.IntVar(frame, fans.DEFAULT_FIXED)
        self.fixed_label = ttk.Label(self.fixed_row, width=6, anchor="e")
        self.fixed_label.pack(side="right")
        self.scale = ttk.Scale(self.fixed_row, from_=0, to=100, orient="horizontal",
                               command=lambda v: self.fixed_label.configure(text=f"{round(float(v))} %"))
        self.scale.pack(side="left", fill="x", expand=True, padx=(0, px(8)))
        self.scale.bind("<ButtonRelease-1>", lambda e: self.changed())

        self.curve_frame = ttk.Frame(editor)
        buttons = ttk.Frame(self.curve_frame)
        buttons.pack(side="bottom", fill="x", pady=(px(4), 0))
        reset = ttk.Button(buttons, text=t("fans.reset_curve"), style="Small.TButton", cursor="hand2",
                           command=self.reset_curve)
        reset.pack(side="right")
        reset.bind("<Enter>", lambda e: window.footer(t("fans.reset_curve.help")))
        self.live = ttk.Label(buttons, style="Foot.TLabel")
        self.live.pack(side="left")
        self.minimum = ttk.Label(self.curve_frame, style="Foot.TLabel", justify="left")
        self.minimum.pack(side="bottom", fill="x")
        self.minimum.bind("<Configure>", lambda e: self.minimum.configure(wraplength=e.width))
        self.canvas = tk.Canvas(self.curve_frame, height=px(190), bg=SURFACE, highlightthickness=0, bd=0)
        self.canvas.pack(side="top", fill="both", expand=True)
        self.canvas.bind("<Configure>", lambda e: self.draw())
        self.canvas.bind("<Motion>", self.on_motion)
        self.canvas.bind("<Leave>", lambda e: self.set_hot(None))
        self.canvas.bind("<ButtonPress-1>", self.on_press)
        self.canvas.bind("<B1-Motion>", self.on_drag)
        self.canvas.bind("<ButtonRelease-1>", self.on_release)
        self.canvas.bind("<Double-Button-1>", self.on_double)
        self.canvas.bind("<Button-3>", self.on_right)

    # --- Data ---
    def items(self):
        return self.app.fans.info

    def item(self):
        return next((i for i in self.items() if i["id"] == self.selected), None)

    def config(self, item):
        return {**fans.default_config(item["kind"]), **self.app.settings["fan_control"].get(item["id"], {})}

    def temperature(self, source):
        current = self.app.current
        if source == "max":
            known = [v for v in (current.get("cpu"), current.get("gpu")) if v is not None]
            return max(known) if known else None
        return current.get(source)

    def hot_limit(self, source):
        s = self.app.settings
        return min(s["cpu_hot_temp"], s["gpu_hot_temp"]) if source == "max" else s[f"{source}_hot_temp"]

    # --- Showing ---
    def update(self):
        """New measurement: list values, note and the live marker in the editor."""
        items = self.items()
        snap = self.app.snapshot()
        values = {s["id"]: s["value"] for hw in snap["hardware"] for s in hw["sensors"]}
        if self.app.fans.notice == "crashed":
            note = t("fans.note.crashed")
        elif not self.app.hardware_ready:
            note = t("window.loading_sensors")
        elif not items:
            note = t("fans.note.none")
        else:
            note = t("fans.note.intro")
        self.note.configure(text=note)

        keys = [i["id"] for i in items]
        if list(self.tree.get_children()) != keys:
            self.tree.delete(*self.tree.get_children())
            for item in items:
                self.tree.insert("", "end", iid=item["id"], text=self.item_name(item))
            if self.selected in keys:
                self.tree.selection_set(self.selected)
        for item in items:
            speeds = [values[i] for i in item["fan_ids"] if i in values]
            controls = [values[i] for i in item["control_ids"] if i in values]
            speed = " / ".join(f"{v:.0f}" for v in speeds) + " RPM" if speeds else "–"
            control = f"{max(controls):.0f} %" if controls else "–"
            self.tree.item(item["id"], values=(speed, control, self.mode_text(item)),
                           tags=("controlled",) if item["software"] else ())
        if items and (self.selected not in keys):
            self.tree.selection_set(keys[0])
            self.select(keys[0])
        elif not items:
            self.select(None)
        self.draw()

    def item_name(self, item):
        return t("fans.gpu_fans", name=item["name"]) if item["kind"] == "gpu" else item["name"]

    def mode_text(self, item):
        cfg = self.config(item)
        if cfg["mode"] == "auto":
            return t("fans.mode.auto")
        note = {"hot": t("fans.state.hot"), "no_temperature": t("fans.state.no_temperature"),
                "error": t("fans.state.error")}.get(item["note"])
        source = t(f"fans.source.{cfg['source']}")
        text = (f"{t('fans.mode.curve')} · {source}" if cfg["mode"] == "curve"
                else f"{t('fans.mode.fixed')} {cfg['fixed']} %")
        return f"{text} – {note}" if note else text

    def select(self, fan_id):
        """Show the editor for a fan (or nothing)."""
        if fan_id == self.selected and fan_id is not None:
            return
        self.selected = fan_id
        item = self.item()
        if not item:
            self.editor.pack_forget()
            return
        self.editor.pack(side="top", fill="both", expand=True, pady=(self.window.px(8), 0))
        cfg = self.config(item)
        self.mode.set(cfg["mode"])
        self.source.set(cfg["source"])
        self.fixed.set(cfg["fixed"])
        self.scale.set(cfg["fixed"])
        self.fixed_label.configure(text=f"{cfg['fixed']} %")
        self.points = [list(p) for p in cfg["curve"]]
        # e.g. Nvidia: not below 30 % while controlled – in Auto the card may stop its fans when idle (quieter)
        self.minimum.configure(text=t("fans.minimum.gpu" if item["kind"] == "gpu" else "fans.minimum",
                                      pct=f"{item['minimum']:.0f}") if item["minimum"] > 0 else "")
        self.show_mode()

    def show_mode(self):
        mode = self.mode.get()
        self.fixed_row.pack_forget()
        self.curve_frame.pack_forget()
        for widget in (self.source_label, *self.source_buttons):
            widget.state(["!disabled"] if mode != "auto" else ["disabled"])
        if mode == "fixed":
            self.fixed_row.pack(side="top", fill="x", pady=(self.window.px(10), 0))
        elif mode == "curve":
            self.curve_frame.pack(side="top", fill="both", expand=True, pady=(self.window.px(6), 0))
            self.draw()

    # --- Changing ---
    def mode_changed(self):
        self.show_mode()
        self.changed()

    def changed(self):
        """Save mode, source, fixed % and curve of the selected fan – the measuring thread applies it right away."""
        item = self.item()
        if not item:
            return
        entry = {"mode": self.mode.get(), "source": self.source.get(), "curve": [list(p) for p in self.points],
                 "fixed": round(float(self.scale.get()))}
        self.app.set_fan_config(item["id"], entry)
        self.tree.item(item["id"], values=(*self.tree.item(item["id"], "values")[:2], self.mode_text(item)))
        self.draw()

    def reset_curve(self):
        item = self.item()
        if item:
            self.points = [list(p) for p in fans.DEFAULT_CURVES[item["kind"]]]
            self.changed()

    # --- Curve editor ---
    def to_xy(self, temp, pct):
        left, top, width, height = self.layout
        return left + (temp - T_MIN) / (T_MAX - T_MIN) * width, top + height - pct / 100 * height

    def from_xy(self, x, y):
        left, top, width, height = self.layout
        temp = T_MIN + (x - left) / width * (T_MAX - T_MIN)
        pct = (top + height - y) / height * 100
        return round(min(max(temp, T_MIN), T_MAX)), round(min(max(pct, 0), 100))

    def draw(self):
        c, px = self.canvas, self.window.px
        c.delete("all")
        item = self.item()
        width, height = c.winfo_width(), c.winfo_height()
        if not item or self.mode.get() != "curve" or width < px(100):
            return
        left, right, top, bottom = px(40), px(20), px(10), px(22)
        self.layout = (left, top, width - left - right, height - top - bottom)
        font = ("Segoe UI", 8)
        source = self.source.get()
        hot = self.hot_limit(source)
        if hot < T_MAX:                          # from the red limit on: always 100 %
            x_hot, _ = self.to_xy(hot, 0)
            x_end, _ = self.to_xy(T_MAX, 0)
            c.create_rectangle(x_hot, top, x_end, top + self.layout[3], fill="#2a2433", outline="")
            label = c.create_text(x_hot + px(4), top + px(2), text=t("fans.red_limit", temp=hot), anchor="nw",
                                  fill=RED, font=font)
            x0, _, x1, _ = c.bbox(label)
            if x1 > x_end:                       # doesn't fit into the red area: left of its border instead
                c.move(label, -(x1 - x0) - px(8), 0)
        for temp in range(T_MIN, T_MAX + 1, 10):  # hairline grid with labels every 20
            x, _ = self.to_xy(temp, 0)
            c.create_line(x, top, x, top + self.layout[3], fill=GRID)
            if temp % 20 == 0:
                c.create_text(x, top + self.layout[3] + px(4), text=f"{temp} °C", anchor="n", fill=SUB, font=font)
        for pct in range(0, 101, 10):
            _, y = self.to_xy(T_MIN, pct)
            c.create_line(left, y, left + self.layout[2], y, fill=GRID)
            if pct % 20 == 0:
                c.create_text(left - px(6), y, text=f"{pct} %", anchor="e", fill=SUB, font=font)
        if item["minimum"] > 0:                  # the fan doesn't go below this
            _, y = self.to_xy(T_MIN, item["minimum"])
            c.create_line(left, y, left + self.layout[2], y, fill=SUB)

        points = sorted(self.points)
        coords = [self.to_xy(T_MIN, points[0][1])]          # flat before the first and after the last point
        coords += [self.to_xy(temp, pct) for temp, pct in points]
        coords.append(self.to_xy(T_MAX, points[-1][1]))
        c.create_line(*[v for xy in coords for v in xy], fill=LINE, width=px(2), capstyle="round", joinstyle="round")

        temp = self.temperature(source)
        if temp is not None:                     # live: current temperature and what the fan gets now
            pct = 100 if temp >= hot else fans.curve_value(points, temp)
            pct = min(max(pct, item["minimum"]), item["maximum"])
            x, y = self.to_xy(min(max(temp, T_MIN), T_MAX), pct)
            c.create_line(x, top, x, top + self.layout[3], fill=SUB)
            c.create_oval(x - px(4), y - px(4), x + px(4), y + px(4), fill=FG, outline=SURFACE, width=px(2))
            applied = f"{item['pct']:.0f} %" if item["software"] and item["pct"] is not None else "–"
            self.live.configure(text=t("fans.live", temp=num(temp, 0), curve=num(pct, 0), applied=applied))
        else:
            self.live.configure(text=t("fans.state.no_temperature"))

        r = px(5)
        for i, (temp, pct) in enumerate(self.points):
            x, y = self.to_xy(temp, pct)
            color = ACTIVE if i in (self.drag, self.hot_point) else LINE
            c.create_oval(x - r, y - r, x + r, y + r, fill=color, outline=SURFACE, width=px(2))

    def point_at(self, x, y):
        """Index of the point near x, y (generous hit area) – or None."""
        if not self.layout:
            return None
        reach = self.window.px(10)
        for i, (temp, pct) in enumerate(self.points):
            px_, py_ = self.to_xy(temp, pct)
            if abs(px_ - x) <= reach and abs(py_ - y) <= reach:
                return i
        return None

    def set_hot(self, index):
        if index != self.hot_point:
            self.hot_point = index
            self.draw()
        if index is not None:
            temp, pct = self.points[index]
            self.window.footer(t("fans.point.help", temp=temp, pct=pct))
        elif self.mode.get() == "curve":
            self.window.footer(t("fans.editor.help"))

    def on_motion(self, event):
        if self.drag is None:
            self.set_hot(self.point_at(event.x, event.y))
            self.canvas.configure(cursor="hand2" if self.hot_point is not None else "")

    def on_press(self, event):
        self.drag = self.point_at(event.x, event.y)

    def on_drag(self, event):
        if self.drag is None or not self.layout:
            return
        temp, pct = self.from_xy(event.x, event.y)
        order = sorted(range(len(self.points)), key=lambda i: self.points[i])
        rank = order.index(self.drag)            # stays between its neighbors (at least 1 °C apart)
        low = self.points[order[rank - 1]][0] + 1 if rank > 0 else T_MIN
        high = self.points[order[rank + 1]][0] - 1 if rank < len(order) - 1 else T_MAX
        self.points[self.drag] = [min(max(temp, low), high), pct]
        self.draw()
        self.window.footer(t("fans.point.help", temp=self.points[self.drag][0], pct=self.points[self.drag][1]))

    def on_release(self, event):
        if self.drag is not None:
            self.drag = None
            self.points.sort()
            self.changed()

    def on_double(self, event):
        if self.point_at(event.x, event.y) is not None or len(self.points) >= fans.MAX_POINTS or not self.layout:
            return
        temp, pct = self.from_xy(event.x, event.y)
        if any(p[0] == temp for p in self.points):
            return
        self.points = sorted(self.points + [[temp, pct]])
        self.changed()

    def on_right(self, event):
        index = self.point_at(event.x, event.y)
        if index is not None and len(self.points) > 2:
            del self.points[index]
            self.hot_point = None
            self.changed()
