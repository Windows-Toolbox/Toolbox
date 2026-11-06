"""
The tab "Drive health" in the detail window – like CrystalDiskInfo: every drive with its health status (Good /
Caution / Bad), temperature, remaining life and power-on hours; for the selected one the key data and the full
SMART table (ID, attribute, current, worst, threshold, raw values) with a colored dot per attribute.

The values come from the measuring thread (read_drive_health() in hardware_monitor.py, every 10 s while this tab is
shown). Status is never shown by color alone – the word is always there too. In privacy mode the serial number is
hidden, also when copying.
"""

from tkinter import ttk

import hardware_data
from hardware_monitor_settings import num, t

SUB, FG = "#8fa3b8", "#e2e8f0"
STATUS_COLORS = {"good": "#3ddc97", "caution": "#f5c451", "bad": "#ff6464", "unknown": SUB}   # like the traffic light


def fmt_gb(gb):
    """48779 GB -> "47.6 TB" (1024-based like the rest of the window)."""
    return f"{num(gb / 1024, 1)} TB" if gb >= 1024 else f"{num(gb, 0)} GB"


def fmt_size(size):
    """Bytes as manufacturers count them: 240057409536 -> "240 GB", 2000398934016 -> "2.0 TB"."""
    return f"{num(size / 1e12, 1)} TB" if size >= 1e12 else f"{size / 1e9:.0f} GB"


def temp_limits(drive):
    """(caution, bad) temperature: what the drive reports itself (NVMe), otherwise like CrystalDiskInfo's defaults."""
    if drive["temp_warning"] and drive["temp_critical"]:
        return drive["temp_warning"], drive["temp_critical"]
    return (55, 65) if drive["ssd"] else (45, 55)


def temp_status(drive):
    if drive["temp"] is None:
        return "unknown"
    caution, bad = temp_limits(drive)
    return "good" if drive["temp"] < caution else "caution" if drive["temp"] < bad else "bad"


def interface(drive):
    bus = drive["bus"].replace("BusType", "").upper()               # "BusTypeSata" -> "SATA"
    kind = "NVMe SSD" if drive["nvme"] else "SSD" if drive["ssd"] else t("health.hdd")
    return kind if drive["nvme"] else f"{bus} · {kind}"


class DriveHealthView:
    """window: the DetailWindow (px, footer, confirm, to_clipboard, private, app). master = frame in the notebook."""

    def __init__(self, frame, window):
        self.window, self.app, self.master = window, window.app, frame
        self.selected = None                     # drive number shown below
        px = window.px

        self.drives = ttk.Treeview(frame, columns=("status", "temp", "life", "hours"), show="tree",
                                   selectmode="browse", height=3)
        self.drives.column("#0", width=px(200), stretch=True)
        for column, width in (("status", 95), ("temp", 55), ("life", 55), ("hours", 75)):
            self.drives.column(column, width=px(width), stretch=False, anchor="e" if column != "status" else "w")
        for level, color in STATUS_COLORS.items():
            self.drives.tag_configure(level, foreground=color)
        self.drives.pack(side="top", fill="x", pady=(px(6), 0))
        self.drives.bind("<<TreeviewSelect>>", lambda e: self.select())
        self.drives.bind("<Motion>", lambda e: window.footer(t("health.list.help")))

        info = ttk.Frame(frame)
        info.pack(side="top", fill="x", pady=(px(8), px(4)))
        self.fields = {}
        names = (("status", "temp"), ("firmware", "interface"), ("serial", "size"), ("hours", "count"),
                 ("reads", "writes"))
        for row, pair in enumerate(names):
            for column, name in enumerate(pair):
                ttk.Label(info, text=t(f"health.field.{name}"), style="Foot.TLabel").grid(
                    row=row, column=column * 2, sticky="w", padx=(0 if column == 0 else px(16), px(8)))
                value = ttk.Label(info, text="–", font=("Segoe UI Semibold", 11) if row == 0 else None)
                value.grid(row=row, column=column * 2 + 1, sticky="w")
                self.fields[name] = value
        info.columnconfigure(1, weight=1)
        info.columnconfigure(3, weight=1)
        button = ttk.Button(info, text=f"⧉  {t('health.copy')}", style="Small.TButton", cursor="hand2",
                            command=self.copy)
        button.grid(row=0, column=4, sticky="ne")
        button.bind("<Enter>", lambda e: window.footer(t("health.copy.help")))
        button.bind("<Leave>", lambda e: window.footer(t("health.hint")))

        table = ttk.Frame(frame)
        table.pack(side="top", fill="both", expand=True, pady=(px(4), px(4)))
        columns = ("id", "attribute", "current", "worst", "threshold", "raw")
        self.smart = ttk.Treeview(table, columns=columns, show="headings", selectmode="extended")
        for column, width, anchor in (("id", 46, "w"), ("attribute", 170, "w"), ("current", 56, "e"),
                                      ("worst", 46, "e"), ("threshold", 66, "e"), ("raw", 104, "e")):
            self.smart.heading(column, text=t(f"health.col.{column}"), anchor=anchor)
            self.smart.column(column, width=px(width), stretch=column == "attribute", anchor=anchor)
        for level, color in STATUS_COLORS.items():
            self.smart.tag_configure(level, foreground=color)
        scroll = ttk.Scrollbar(table, orient="vertical", command=self.smart.yview)
        self.smart.configure(yscrollcommand=scroll.set)
        scroll.pack(side="right", fill="y")
        self.smart.pack(side="left", fill="both", expand=True)
        self.smart.bind("<Motion>", lambda e: window.footer(t("health.hint")))

    def drive(self):
        return next((d for d in self.app.drive_health or [] if d["number"] == self.selected), None)

    def update(self):
        drives = self.app.drive_health
        keys = [str(d["number"]) for d in drives or []]
        if list(self.drives.get_children()) != keys:
            self.drives.delete(*self.drives.get_children())
            for d in drives or []:
                self.drives.insert("", "end", iid=str(d["number"]), text=f"{d['number']}  {d['model']}")
            if not drives:
                self.drives.insert("", "end", iid="none", tags=("unknown",),
                                   text=t("health.reading") if drives is None else t("health.none"))
        for d in drives or []:
            status = d["status"].lower()
            life = f"{num(d['life'], 0)} %" if d["life"] is not None else ""
            temp = f"{num(d['temp'], 0)} °C" if d["temp"] is not None else "–"
            self.drives.item(str(d["number"]), tags=(status,),
                             values=(f"●  {t(f'health.status.{status}')}", temp, life, f"{d['hours']:,} h"))
        if drives and str(self.selected) not in keys:
            self.selected = drives[0]["number"]
            self.drives.selection_set(str(self.selected))
        self.show_drive()

    def select(self):
        selection = self.drives.selection()
        if selection and selection[0] != "none":
            self.selected = int(selection[0])
            self.show_drive()

    def show_drive(self):
        d = self.drive()
        if not d:
            for value in self.fields.values():
                value.configure(text="–", foreground=FG)
            self.smart.delete(*self.smart.get_children())
            return
        status = d["status"].lower()
        text = t(f"health.status.{status}")
        if d["life"] is not None:
            text = t("health.with_life", status=text, life=num(d["life"], 0))
        self.fields["status"].configure(text=f"●  {text}", foreground=STATUS_COLORS.get(status, SUB))
        level = temp_status(d)
        self.fields["temp"].configure(text=f"{num(d['temp'], 0)} °C" if d["temp"] is not None else "–",
                                      foreground=STATUS_COLORS[level] if level != "unknown" else SUB)
        serial = hardware_data.shown("serial_number", d["serial"], self.window.private()) or "–"
        values = {"firmware": d["firmware"] or "–", "interface": interface(d), "serial": serial,
                  "size": fmt_size(d["size"]), "hours": f"{d['hours']:,} h", "count": f"{d['count']:,}",
                  "reads": fmt_gb(d["reads"]) if d["reads"] is not None else "–",
                  "writes": fmt_gb(d["writes"]) if d["writes"] is not None else "–"}
        for name, value in values.items():
            self.fields[name].configure(text=value)

        rows = [(f"{a['id']:02X}", a["name"], a["current"], a["worst"], a["threshold"], f"{a['raw']:012X}",
                 a["status"]) for a in d["attributes"]]
        keys = [f"{i}" for i in range(len(rows))]
        if list(self.smart.get_children()) != keys:
            self.smart.delete(*self.smart.get_children())
            for key in keys:
                self.smart.insert("", "end", iid=key)
        for key, (id_, name, current, worst, threshold, raw, status) in zip(keys, rows):
            values = (f"●  {id_}", name, *(("", "", "") if d["nvme"] else (current, worst, threshold)), raw)
            self.smart.item(key, values=values, tags=(status,))

    def copy(self):
        """The selected drive as text: key data and SMART table (serial number hidden in privacy mode)."""
        d = self.drive()
        if not d:
            return
        lines = [f"{d['number']}  {d['model']}"]
        for name in ("status", "temp", "firmware", "interface", "serial", "size", "hours", "count", "reads", "writes"):
            # as shown – the serial number is already ████████ in privacy mode
            lines.append(f"  {t(f'health.field.{name}')}: {self.fields[name].cget('text').lstrip('● ')}")
        header = [t(f"health.col.{c}") for c in ("id", "attribute", "current", "worst", "threshold", "raw")]
        lines += ["", "\t".join(header)]
        for a in d["attributes"]:
            values = ("", "", "") if d["nvme"] else (a["current"], a["worst"], a["threshold"])
            lines.append("\t".join(str(v) for v in (f"{a['id']:02X}", a["name"], *values, f"{a['raw']:012X}")))
        self.window.to_clipboard("\n".join(lines) + "\n", t("health.copied", model=d["model"]))
