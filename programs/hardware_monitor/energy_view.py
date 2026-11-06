"""
The tab "Power" in the detail window: current power, totals and a bar chart of the logged consumption
(per hour for the last 24 hours, per day for the last 30 days, per month for the last 12 months).

Drawn on a tk.Canvas: one series, so no legend – the title above the chart names it. Bars at most 24 px wide with a
4 px rounded top and a 2 px gap, hairline gridlines, only the highest bar labeled; everything else is in the footer
while hovering over a bar and in "Copy table" (tab-separated, e.g. for Excel). The costs are kWh × price from the
settings – a second unit, so they are not a second axis but in the tiles, the footer and the table.
"""

import math
import tkinter as tk
from datetime import datetime
from tkinter import ttk

import energy
from energy import fmt_duration, fmt_energy
from hardware_monitor_settings import num, t

BAR = "#25a774"          # one step darker than the accent – passes the lightness band on the dark chart surface
BAR_HOVER = "#3ddc97"    # accent for the bar under the mouse
GRID = "#213443"         # one step off the surface, hairline
DAYS = ("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
MONTHS = ("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")
VIEWS = ("day", "month", "year")


def nice_step(value):
    """A round step for the axis (1, 2, 2.5 or 5 × 10^n) so that about 3 gridlines fit up to value."""
    if value <= 0:
        return 1
    raw = value / 3
    power = 10 ** math.floor(math.log10(raw))
    for factor in (1, 2, 2.5, 5, 10):
        if raw <= factor * power:
            return factor * power
    return 10 * power


def period(start, view):
    """Thu 6 Nov, 14:00–15:00 / Thu 6 Nov / November 2026."""
    day = f"{DAYS[start.weekday()]} {start.day} {MONTHS[start.month - 1]}"
    if view == "day":
        return f"{day}, {start.hour:02d}:00–{(start.hour + 1) % 24:02d}:00"
    if view == "month":
        return day
    return f"{MONTHS[start.month - 1]} {start.year}"


class EnergyView:
    """window: the DetailWindow (px, footer, confirm, to_clipboard, app). master is the frame in the notebook."""

    def __init__(self, frame, window):
        self.window, self.app, self.master = window, window.app, frame
        self.view = tk.StringVar(frame, "day")
        self.hover = None                        # index of the bar under the mouse
        self.data = []                           # buckets of the current view
        self.bar_items = []                      # canvas items of the bars (for the hover color)
        self.layout = None                       # (left, slot width, count) for finding the bar under the mouse
        px = window.px

        bar = ttk.Frame(frame)
        bar.pack(side="top", fill="x", pady=(px(6), px(4)))
        for view in VIEWS:
            sub_tab = ttk.Radiobutton(bar, text=t(f"power.view.{view}"), value=view, variable=self.view,
                                      style="SubTab.Toolbutton", cursor="hand2", command=self.update)
            sub_tab.pack(side="left")
        button = ttk.Button(bar, text=f"⧉  {t('power.copy_table')}", style="Small.TButton", cursor="hand2",
                            command=self.copy_table)
        button.pack(side="right")
        button.bind("<Enter>", lambda e: window.footer(t("power.copy_table.help")))
        button.bind("<Leave>", lambda e: window.footer(t("power.hint")))

        tiles = ttk.Frame(frame)
        tiles.pack(side="top", fill="x", pady=(px(4), px(2)))
        self.tiles = {}
        for column, name in enumerate(("now", "today", "month", "uptime")):
            tile = ttk.Frame(tiles, style="Tile.TFrame", padding=(px(10), px(6)))
            tile.grid(row=0, column=column, sticky="nsew", padx=(0 if column == 0 else px(6), 0))
            tiles.columnconfigure(column, weight=1, uniform="tiles")
            label = ttk.Label(tile, text=t(f"power.tile.{name}"), style="TileLabel.TLabel")
            value = ttk.Label(tile, text="–", style="TileValue.TLabel")
            sub = ttk.Label(tile, text="", style="TileLabel.TLabel")
            for widget in (label, value, sub):
                widget.pack(anchor="w")
            for widget in (tile, label, value, sub):
                widget.bind("<Enter>", lambda e, n=name: window.footer(self.tile_help(n)))
                widget.bind("<Leave>", lambda e: window.footer(t("power.hint")))
            self.tiles[name] = (value, sub)

        self.note = ttk.Label(frame, style="Foot.TLabel", justify="left")
        self.note.pack(side="top", fill="x", pady=(px(6), 0))
        self.note.bind("<Configure>", lambda e: self.note.configure(wraplength=e.width))
        self.title = ttk.Label(frame, style="Foot.TLabel")
        self.title.pack(side="top", anchor="w", pady=(px(8), px(2)))

        self.canvas = tk.Canvas(frame, height=px(210), bg="#152736", highlightthickness=0, bd=0)
        self.canvas.pack(side="top", fill="both", expand=True, pady=(0, px(4)))
        self.canvas.bind("<Configure>", lambda e: self.draw())
        self.canvas.bind("<Motion>", self.on_motion)
        self.canvas.bind("<Leave>", lambda e: self.set_hover(None))

    # --- Values ---
    def settings(self):
        return self.app.settings

    def cost(self, wh):
        return energy.fmt_cost(wh, self.settings())

    def update(self):
        """New measurement or other view: tiles, note and chart anew."""
        hours = self.app.energy.snapshot()
        totals = energy.totals(hours)
        power = self.app.power_now
        value, sub = self.tiles["now"]
        if power:
            value.configure(text=f"{'' if power['battery'] is not None else '≈ '}{num(power['watts'], 0)} W")
            parts = [f"CPU {num(power['cpu'], 0)}" if power["cpu"] is not None else "",
                     f"GPU {num(power['gpu'], 0)}" if power["gpu"] is not None else ""]   # watts: see the value
            sub.configure(text=t("power.on_battery") if power["battery"] is not None
                          else " · ".join(p for p in parts if p))
        else:
            value.configure(text="–")
            sub.configure(text=t("power.measuring"))
        for name in ("today", "month"):
            wh, _ = totals[name]
            value, sub = self.tiles[name]
            value.configure(text=fmt_energy(wh, "kWh") if wh else "–")
            sub.configure(text=self.cost(wh) if wh else "")
        uptime = energy.uptime_seconds()
        value, sub = self.tiles["uptime"]
        value.configure(text=fmt_duration(uptime))
        started = datetime.now().timestamp() - uptime
        sub.configure(text=t("power.since", time=datetime.fromtimestamp(started).strftime("%H:%M")))

        logging_on = self.settings()["energy_logging"]
        if not logging_on:
            note = t("power.note.off")
        elif not hours:
            note = t("power.note.empty")
        else:
            note = t("power.note.on", since=totals["since"])
        self.note.configure(text=note)
        self.title.configure(text=t(f"power.chart.{self.view.get()}"))
        self.data = energy.buckets(hours, self.view.get())
        self.draw()

    def tile_help(self, name):
        """Footer text for a tile."""
        power, s = self.app.power_now, self.settings()
        if name == "now" and power and power["battery"] is not None:
            return t("power.help.battery")
        if name == "now" and power:
            return t("power.help.now", cpu=num(power["cpu"] or 0, 0), gpu=num(power["gpu"] or 0, 0),
                     other=s["energy_other_watts"], efficiency=s["energy_psu_efficiency"],
                     watts=num(power["watts"], 0))
        return t(f"power.help.{name}")

    # --- Chart ---
    def draw(self):
        c, px = self.canvas, self.window.px
        c.delete("all")
        self.bar_items = []
        width, height = c.winfo_width(), c.winfo_height()
        if width < px(100) or not self.data:
            return
        view = self.view.get()
        unit = "Wh" if view == "day" else "kWh"
        scale = 1 if unit == "Wh" else 1000
        values = [wh / scale for _, _, wh, _ in self.data]
        step = nice_step(max(values))
        top = step * max(1, -(-max(values) // step)) if max(values) > 0 else step * 3
        left, right, top_margin, bottom = px(52), px(8), px(18), px(22)
        plot_h, plot_w = height - top_margin - bottom, width - left - right
        base = top_margin + plot_h
        font = ("Segoe UI", 8)

        tick = 0.0
        while tick <= top + step / 1000:         # gridlines and their labels
            y = base - tick / top * plot_h
            c.create_line(left, y, width - right, y, fill=GRID)
            decimals = 0 if step >= 1 else 1 if step >= 0.1 else 2
            c.create_text(left - px(6), y, text=f"{num(tick, decimals)} {unit}", anchor="e", fill="#8fa3b8",
                          font=font)
            tick += step

        count = len(values)
        slot = plot_w / count
        bar_w = max(px(2), min(px(24), slot - px(2)))
        radius = min(px(4), bar_w / 2)
        highest = max(range(count), key=lambda i: values[i])
        for i, (value, (start, _, _, _)) in enumerate(zip(values, self.data)):
            x0 = left + i * slot + (slot - bar_w) / 2
            x1 = x0 + bar_w
            y = base - value / top * plot_h
            if value > 0:
                y = min(y, base - px(1))         # tiny values stay visible as a sliver
                item = self.rounded_top(x0, y, x1, base, radius,
                                        BAR_HOVER if i == self.hover else BAR)
            else:
                item = None
            self.bar_items.append(item)
            if i == highest and value > 0:       # only the highest bar gets its value
                c.create_text((x0 + x1) / 2, y - px(3), text=fmt_energy(value * scale, unit), anchor="s",
                              fill="#e2e8f0", font=font)
            label = self.x_label(start, i, count, view)
            if label:
                c.create_text((x0 + x1) / 2, base + px(4), text=label, anchor="n", fill="#8fa3b8", font=font)
        self.layout = (left, slot, count)

    def rounded_top(self, x0, y0, x1, y1, r, color):
        """Bar with a rounded top (radius r) and a square bottom on the baseline."""
        r = min(r, (y1 - y0), (x1 - x0) / 2)
        points = [x0, y1]
        for cx, start in ((x0 + r, 180), (x1 - r, 90)):          # quarter circles top left and top right
            for step in range(7):
                angle = math.radians(start - step * 15)
                points += [cx + r * math.cos(angle), y0 + r - r * math.sin(angle)]
        points += [x1, y1]
        return self.canvas.create_polygon(points, fill=color, outline="")

    @staticmethod
    def x_label(start, i, count, view):
        if view == "day":
            return f"{start.hour:02d}:00" if start.hour % 6 == 0 else ""
        if view == "month":
            return f"{start.day} {MONTHS[start.month - 1]}" if (count - 1 - i) % 7 == 0 else ""
        return MONTHS[start.month - 1]

    # --- Hover ---
    def on_motion(self, event):
        if not self.layout:
            return
        left, slot, count = self.layout
        index = int((event.x - left) // slot)
        self.set_hover(index if 0 <= index < count and event.x >= left else None)

    def set_hover(self, index):
        if index == self.hover:
            return
        for i in (self.hover, index):
            if i is not None and i < len(self.bar_items) and self.bar_items[i]:
                self.canvas.itemconfigure(self.bar_items[i], fill=BAR_HOVER if i == index else BAR)
        self.hover = index
        if index is None or index >= len(self.data):
            self.window.footer(t("power.hint"))
            return
        start, end, wh, seconds = self.data[index]
        view = self.view.get()
        if seconds:
            text = t("power.bar", period=period(start, view), energy=fmt_energy(wh, "Wh" if view == "day" else "kWh"),
                     cost=self.cost(wh), measured=fmt_duration(seconds))
        else:
            text = t("power.bar.empty", period=period(start, view))
        self.window.footer(text)

    # --- Table view ---
    def copy_table(self):
        """The bars of the current view as a tab-separated table (pastes into Excel), plus the totals."""
        view = self.view.get()
        s = self.settings()
        lines = ["\t".join((t("power.table.period"), "kWh", f"{t('power.table.cost')} ({s['energy_currency']})",
                            t("power.table.measured")))]
        for start, _, wh, seconds in self.data:
            lines.append("\t".join((period(start, view), num(wh / 1000, 3), num(wh / 1000 * s["energy_price"], 2),
                                    fmt_duration(seconds) if seconds else "")))
        self.window.to_clipboard("\n".join(lines) + "\n", t("power.table.copied", n=len(self.data)))
