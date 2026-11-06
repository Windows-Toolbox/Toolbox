"""
Fixed data of the hardware for the tab "Hardware data": manufacturer, model, vendor and product IDs, serial numbers,
drivers. Read via WMI (Windows Management Instrumentation) through pythonnet – once when the window opens, not
continuously. Needs no admin rights and no LibreHardwareMonitor.

Categories and fields are IDs here (e.g. "graphics_card", "vendor_id"); their names and explanations are in
i18n.json (hw.category.…, hw.field.…, hw.explain.…) and are only filled in when displaying or copying.

To try it out:  python hardware_data.py   -> prints everything the tab would show
"""

import ctypes
import logging
import platform
import re
import uuid
from collections import Counter
from datetime import date

from hardware_monitor_settings import fmt_date, num, t

# PCI vendor IDs (excerpt: the most common ones in desktop PCs)
PCI_VENDORS = {
    "10DE": "Nvidia", "1002": "AMD", "1022": "AMD", "8086": "Intel", "10EC": "Realtek", "144D": "Samsung",
    "1043": "Asus", "1462": "MSI", "1458": "Gigabyte", "1849": "ASRock", "15B7": "SanDisk / WD", "1B4B": "Marvell",
    "14E4": "Broadcom", "168C": "Qualcomm Atheros", "1969": "Qualcomm Atheros", "1E0F": "Kioxia", "1179": "Toshiba",
    "1987": "Phison", "126F": "Silicon Motion", "1CC1": "ADATA", "2646": "Kingston", "C0A9": "Crucial",
    "1344": "Micron", "3842": "EVGA", "1DA2": "Sapphire", "174B": "Sapphire", "196E": "PNY", "19DA": "Zotac",
    "1682": "XFX", "148C": "PowerColor", "1569": "Palit", "10B0": "Gainward", "1028": "Dell", "103C": "HP",
    "17AA": "Lenovo", "1025": "Acer", "1B21": "ASMedia", "1D6A": "Aquantia", "14F1": "Conexant / Senary",
    "152D": "Quanta", "1D72": "Xiaomi", "19E5": "Huawei",
}
# USB vendor IDs (excerpt)
USB_VENDORS = {
    "046D": "Logitech", "1532": "Razer", "1235": "Focusrite", "056A": "Wacom", "0DB0": "MSI", "1462": "MSI",
    "2E1A": "Insta360", "045E": "Microsoft", "1B1C": "Corsair", "1038": "SteelSeries", "0951": "Kingston",
    "03F0": "HP", "04F2": "Chicony", "05AC": "Apple", "054C": "Sony", "0781": "SanDisk", "0BDA": "Realtek",
    "8087": "Intel", "046A": "Cherry", "0B05": "Asus", "26CE": "ASRock", "048D": "ITE", "1B21": "ASMedia",
    "2109": "VIA Labs", "05E3": "Genesys Logic", "0A12": "Cambridge Silicon Radio", "1050": "Yubico",
    "0B95": "ASIX", "18D1": "Google", "04E8": "Samsung", "2717": "Xiaomi",
}
# RAM manufacturer from the start of the part number – Windows often only reports "Unknown" as the manufacturer
RAM_PART_NUMBERS = (
    (r"F[345]-", "G.Skill"), (r"CM", "Corsair"), (r"KF|KHX|KVR|KCP", "Kingston"), (r"BL|CT", "Crucial"),
    (r"M[34]\d\d", "Samsung"), (r"HM", "SK Hynix"), (r"MTA", "Micron"), (r"A[XD][45]", "ADATA"),
)
RAM_TYPES = {20: "DDR", 21: "DDR2", 24: "DDR3", 26: "DDR4", 29: "LPDDR3", 30: "LPDDR4", 34: "DDR5", 35: "LPDDR5"}
# Values starting with "hw.value." are IDs from i18n.json, the rest are names that are the same everywhere (see display())
RAM_FORM_FACTORS = {8: "hw.value.dimm", 12: "hw.value.sodimm"}     # Win32_PhysicalMemory.FormFactor
NET_KINDS = {14: "hw.value.lan", 9: "WLAN", 1: "WLAN", 8: "hw.value.mobile", 10: "Bluetooth"}   # NdisPhysicalMedium
BUS_TYPES = {1: "SCSI", 3: "ATA", 7: "USB", 8: "RAID", 9: "iSCSI", 10: "SAS", 11: "SATA", 12: "hw.value.sd_card",
             17: "NVMe"}
MEDIA_TYPES = {3: "hw.value.hdd", 4: "SSD"}
CPU_VENDORS = {"AuthenticAMD": "AMD", "GenuineIntel": "Intel"}
PLACEHOLDERS = {"", "unknown", "default string", "to be filled by o.e.m.", "none", "not available", "00000000"}

# Privacy mode: these fields (and the computer name when copying everything) are shown and copied as HIDDEN – e.g.
# for screenshots or forum posts. The real values stay in memory; only display and copying replace them.
SENSITIVE_FIELDS = {"serial_number", "mac_address", "processor_id"}
HIDDEN = "████████"


def shown(field, value, private):
    """The value as it is shown and copied: HIDDEN for sensitive fields while privacy mode is on."""
    return HIDDEN if private and field in SENSITIVE_FIELDS else value


# ---------- WMI ----------
def query(wql, scope=r"root\cimv2"):
    """Runs a WMI query. Result: list of dicts (values as text or None); empty on errors."""
    try:
        import clr  # pythonnet – loaded in the Hardware Monitor anyway
        clr.AddReference("System.Management")
        from System.Management import ManagementObjectSearcher
        with ManagementObjectSearcher(scope, wql) as searcher:
            return [{p.Name: None if p.Value is None else str(p.Value) for p in obj.Properties}
                    for obj in searcher.Get()]
    except Exception:
        logging.exception("WMI query failed: %s", wql)
        return []


# ---------- DXCore (Windows graphics interface): integrated or dedicated GPU? ----------
class GUID(ctypes.Structure):
    _fields_ = [("bytes", ctypes.c_ubyte * 16)]

    def __init__(self, text):
        super().__init__()
        ctypes.memmove(self.bytes, uuid.UUID(text).bytes_le, 16)


def com_method(obj, number, result, *argtypes):
    """Method no. `number` of a COM object, counted as in dxcore_interface.h (0–2: QueryInterface, AddRef, Release)."""
    vtable = ctypes.cast(obj, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
    return lambda *args: ctypes.WINFUNCTYPE(result, ctypes.c_void_p, *argtypes)(vtable[number])(obj, *args)


def gpu_kinds():
    """(vendor ID, product ID, subsystem) -> "Integrated (in the processor)" / "External (eGPU)" / "Dedicated".
    The name doesn't tell ("Intel Arc Graphics" is integrated, "Intel Arc A770" a card)."""
    from ctypes import POINTER, byref, c_bool, c_size_t, c_uint32, c_void_p
    HRESULT = ctypes.HRESULT
    factory, adapters = c_void_p(), c_void_p()
    ctypes.WinDLL("dxcore").DXCoreCreateAdapterFactory(byref(GUID("78ee5945-c36e-4b13-a669-005dd11c0f06")),
                                                       byref(factory))
    com_method(factory, 3, HRESULT, c_uint32, POINTER(GUID), POINTER(GUID), POINTER(c_void_p))(    # CreateAdapterList
        1, byref(GUID("248e2800-a793-4724-abaa-23a6de1be090")),         # all GPUs that can compute
        byref(GUID("526c7776-40e9-459b-b711-f32ad76dfc28")), byref(adapters))
    kinds = {}
    for i in range(com_method(adapters, 4, c_uint32)()):              # GetAdapterCount
        adapter = c_void_p()
        com_method(adapters, 3, HRESULT, c_uint32, POINTER(GUID), POINTER(c_void_p))(              # GetAdapter
            i, byref(GUID("f0db4c7f-fe5a-42a2-bd62-f2a6cf6fc83e")), byref(adapter))
        get_property = com_method(adapter, 6, HRESULT, c_uint32, c_size_t, c_void_p)               # GetProperty
        ids, integrated, detachable = (c_uint32 * 4)(), c_bool(), c_bool()
        get_property(3, ctypes.sizeof(ids), ids)                        # HardwareID: vendor, product, subsystem
        get_property(12, 1, byref(integrated))                          # IsIntegrated
        get_property(13, 1, byref(detachable))                          # IsDetachable
        kinds[(f"{ids[0]:04X}", f"{ids[1]:04X}", f"{ids[2]:08X}")] = (
            t("hw.value.integrated") if integrated.value else t("hw.value.external") if detachable.value
            else t("hw.value.dedicated"))
        com_method(adapter, 2, c_uint32)()                              # Release
    com_method(adapters, 2, c_uint32)()
    com_method(factory, 2, c_uint32)()
    return kinds


# ---------- Names in the chosen language ----------
def category_name(category):
    """"graphics_card" -> "Graphics card" (or "Grafikkarte" in German)."""
    return t(f"hw.category.{category}")


def field_name(field):
    """"vendor_id" -> "Vendor ID" (or "Hersteller-ID" in German)."""
    return t(f"hw.field.{field}")


def explanation(field):
    """What a field means (footer of the window) – or None if there is no explanation."""
    return t(f"hw.explain.{field}") if f"hw.explain.{field}" in t else None


def display(entry):
    """Entry from RAM_FORM_FACTORS etc.: an ID gets translated, a name stays; None stays None."""
    return t(entry) if entry and entry in t else entry


# ---------- Helpers ----------
def clean(text):
    """Cleaned-up text – or None for empty values and placeholders like "To Be Filled By O.E.M."."""
    text = (text or "").strip()
    return None if text.lower() in PLACEHOLDERS else text


def with_name(code, table):
    """"10DE" -> "10DE (Nvidia)", unknown codes stay as they are."""
    return f"{code} ({table[code]})" if code in table else code


def to_int(text):
    try:
        return int(text)
    except (TypeError, ValueError):
        return None


def wmi_date(text):
    """WMI date "20210303000000.000000+000" -> "2021-03-03" (German: "03.03.2021")."""
    m = re.match(r"(\d{4})(\d\d)(\d\d)", text or "")
    return fmt_date(int(m[1]), int(m[2]), int(m[3])) if m else None


def hardware_id(pnp):
    """"PCI\\VEN_10DE&DEV_2805&…\\4&1FC9…" -> "PCI\\VEN_10DE&DEV_2805&…" (without the part that only names the slot)."""
    return pnp.rsplit("\\", 1)[0] if pnp and pnp.count("\\") >= 2 else pnp


def pci_fields(pnp):
    """Vendor, product and subsystem ID from a PCI or HD audio ID."""
    m = re.search(r"VEN_([0-9A-F]{4})&DEV_([0-9A-F]{4})(?:&SUBSYS_([0-9A-F]{8}))?(?:&REV_([0-9A-F]+))?",
                  pnp or "", re.I)
    if not m:
        return []
    ven, dev, subsys, rev = (g.upper() if g else None for g in m.groups())
    fields = [("vendor_id", with_name(ven, PCI_VENDORS)), ("product_id", dev)]
    if subsys and subsys != "00000000":
        # PCI: SUBSYS_<model><vendor>, audio codecs the other way round: SUBSYS_<vendor><model>
        codec = re.match(r"HDAUDIO\\|INTELAUDIO\\FUNC_", pnp, re.I)
        sub_dev, sub_ven = (subsys[4:], subsys[:4]) if codec else (subsys[:4], subsys[4:])
        fields += [("board_vendor", with_name(sub_ven, PCI_VENDORS)), ("subsystem_id", sub_dev)]
    if rev:
        fields.append(("revision", rev))
    return fields + [("hardware_id", hardware_id(pnp))]


def usb_ids(pnp):
    m = re.search(r"VID_([0-9A-F]{4})&PID_([0-9A-F]{4})", pnp or "", re.I)
    return (m[1].upper(), m[2].upper()) if m else None


def id_fields(pnp):
    """IDs from a PCI or USB ID (network adapters can be either)."""
    ids = usb_ids(pnp)
    if ids:
        return [("vendor_id", with_name(ids[0], USB_VENDORS)), ("product_id", ids[1]),
                ("hardware_id", hardware_id(pnp))]
    return pci_fields(pnp)


def non_empty(fields):
    return [(name, value) for name, value in fields if value]


# ---------- The individual categories ----------
def processors():
    devices = []
    for p in query("SELECT Name, Manufacturer, ProcessorId, NumberOfCores, NumberOfLogicalProcessors, "
                   "MaxClockSpeed, SocketDesignation, L2CacheSize, L3CacheSize FROM Win32_Processor"):
        vendor = clean(p["Manufacturer"])
        l3 = to_int(p["L3CacheSize"])
        devices.append((clean(p["Name"]) or t("hw.name.processor"), non_empty([
            ("manufacturer", CPU_VENDORS.get(vendor, vendor)),
            ("cores_threads", f"{p['NumberOfCores']} / {p['NumberOfLogicalProcessors']}"),
            ("base_clock", f"{p['MaxClockSpeed']} MHz" if to_int(p["MaxClockSpeed"]) else None),
            ("l3_cache", f"{l3 // 1024} MB" if l3 else None),
            ("socket", clean(p["SocketDesignation"])),
            ("processor_id", clean(p["ProcessorId"])),
        ])))
    return devices


def graphics_cards():
    try:
        kinds = gpu_kinds()
    except Exception:                            # e.g. Windows 10 before version 2004 (without dxcore.dll): field missing
        logging.exception("DXCore: kind of graphics cards not readable")
        kinds = {}
    devices = []
    for g in query("SELECT Name, PNPDeviceID, DriverVersion, DriverDate FROM Win32_VideoController"):
        pnp = (g["PNPDeviceID"] or "").upper()
        if not pnp.startswith("PCI\\"):
            continue                             # leave out virtual displays (remote access, streaming)
        m = re.search(r"VEN_(\w{4})&DEV_(\w{4})(?:&SUBSYS_(\w{8}))?", pnp)
        kind = m and (kinds.get(m.groups()) or next((k for ids, k in kinds.items() if ids[:2] == m.groups()[:2]), None))
        devices.append((clean(g["Name"]) or t("hw.name.graphics_card"), non_empty(
            [("kind", kind)] + pci_fields(g["PNPDeviceID"])
            + [("driver_version", clean(g["DriverVersion"])), ("driver_date", wmi_date(g["DriverDate"]))])))
    return devices


def motherboards():
    devices = []
    for b in query("SELECT Manufacturer, Product, Version, SerialNumber FROM Win32_BaseBoard"):
        vendor, model = clean(b["Manufacturer"]), clean(b["Product"])
        fields = [("manufacturer", vendor), ("model", model), ("version", clean(b["Version"])),
                  ("serial_number", clean(b["SerialNumber"]))]
        for bios in query("SELECT Manufacturer, SMBIOSBIOSVersion, ReleaseDate FROM Win32_BIOS"):
            fields += [("bios_vendor", clean(bios["Manufacturer"])),
                       ("bios_version", clean(bios["SMBIOSBIOSVersion"])), ("bios_date", wmi_date(bios["ReleaseDate"]))]
        devices.append((" ".join(filter(None, (vendor, model))) or t("hw.name.motherboard"), non_empty(fields)))
    return devices


def memory_modules():
    devices, soldered = [], {}
    for r in query("SELECT Manufacturer, PartNumber, SerialNumber, BankLabel, DeviceLocator, Capacity, Speed, "
                   "ConfiguredClockSpeed, SMBIOSMemoryType, FormFactor FROM Win32_PhysicalMemory"):
        part = clean(r["PartNumber"])
        vendor = vendor_name = clean(r["Manufacturer"])
        if not vendor and part:
            vendor_name = next((name for pattern, name in RAM_PART_NUMBERS if re.match(pattern, part)), None)
            vendor = t("hw.value.from_part_number", vendor=vendor_name) if vendor_name else None
        size = to_int(r["Capacity"]) or 0
        ram_type = RAM_TYPES.get(to_int(r["SMBIOSMemoryType"]))
        configured, rated = to_int(r["ConfiguredClockSpeed"]), to_int(r["Speed"])
        name = " ".join(filter(None, (vendor_name, part)))
        if ram_type and ram_type.startswith("LP"):   # LPDDR is always soldered and comes per memory channel -> combine
            group = soldered.setdefault(name or t("hw.name.memory"), {
                "manufacturer": vendor, "part_number": part, "size": 0, "type": ram_type,
                "speed": f"{configured} MT/s" if configured else None, "form_factor": t("hw.value.soldered"),
                "channels": 0})
            group["size"] += size
            group["channels"] += 1
            continue
        devices.append((name or t("hw.name.memory_module"), non_empty([
            ("manufacturer", vendor),
            ("part_number", part),
            ("size", f"{size // 1024 ** 3} GB" if size else None),
            ("type", ram_type),
            ("form_factor", display(RAM_FORM_FACTORS.get(to_int(r["FormFactor"])))),
            ("speed", f"{configured} MT/s" if configured else None),
            ("rated_speed", f"{rated} MT/s" if rated and rated != configured else None),
            ("slot", " / ".join(filter(None, (clean(r["BankLabel"]), clean(r["DeviceLocator"]))))),
            ("serial_number", clean(r["SerialNumber"])),
        ])))
    for name, group in soldered.items():
        group["size"] = f"{group['size'] // 1024 ** 3} GB" if group["size"] else None
        group["channels"] = str(group["channels"])
        devices.insert(0, (name, non_empty(list(group.items()))))
    return devices


def drives():
    devices = []
    for d in sorted(query("SELECT DeviceId, FriendlyName, SerialNumber, FirmwareVersion, Size, BusType, MediaType "
                          "FROM MSFT_PhysicalDisk", scope=r"root\Microsoft\Windows\Storage"),
                    key=lambda d: to_int(d["DeviceId"]) or 0):
        size = to_int(d["Size"])
        devices.append((clean(d["FriendlyName"]) or t("hw.name.drive"), non_empty([
            ("kind", display(MEDIA_TYPES.get(to_int(d["MediaType"])))),
            ("interface", display(BUS_TYPES.get(to_int(d["BusType"])))),
            ("size", None if not size else f"{num(size / 1e12, 1)} TB" if size >= 1e12
             else f"{size / 1e9:.0f} GB"),
            ("serial_number", (clean(d["SerialNumber"]) or "").rstrip(".") or None),
            ("firmware", clean(d["FirmwareVersion"])),
            ("disk_number", d["DeviceId"]),
        ])))
    return devices


def network_adapters():
    # The kind (LAN, WLAN, …) is only in the newer class MSFT_NetAdapter – Win32_NetworkAdapter also reports WLAN
    # as "Ethernet". They are matched via the device ID.
    kinds = {(a["PnPDeviceID"] or "").upper(): display(NET_KINDS.get(to_int(a["NdisPhysicalMedium"])))
             for a in query("SELECT PnPDeviceID, NdisPhysicalMedium FROM MSFT_NetAdapter",
                            scope=r"root\StandardCimv2")}
    devices = []
    for n in query("SELECT Name, PNPDeviceID, MACAddress FROM Win32_NetworkAdapter WHERE PhysicalAdapter = TRUE"):
        pnp = n["PNPDeviceID"] or ""
        if not re.match(r"(PCI|USB)\\", pnp, re.I):
            continue                             # leave out virtual adapters (VPN, Hyper-V, …)
        devices.append((clean(n["Name"]) or t("hw.name.network_adapter"), non_empty(
            [("kind", kinds.get(pnp.upper()))] + id_fields(pnp) + [("mac_address", clean(n["MACAddress"]))])))
    return devices


def audio_devices():
    devices = []
    for s in query("SELECT Name, PNPDeviceID FROM Win32_SoundDevice"):
        pnp = s["PNPDeviceID"] or ""
        # INTELAUDIO\FUNC_: codec behind Intel Smart Sound (laptops) – the other INTELAUDIO entries are only
        # endpoints of this controller (microphone, Bluetooth, USB)
        if not re.match(r"(PCI|HDAUDIO|INTELAUDIO\\FUNC_)", pnp, re.I):
            continue                             # USB audio is listed under USB devices, virtual ones are missing
        name = clean(s["Name"]) or t("hw.name.audio_device")
        m = re.search(r"VEN_10EC&DEV_0([0-9A-F]{3})", pnp, re.I)
        if m and name.startswith("High Definition Audio"):
            name = f"Realtek ALC{m[1]}"           # Realtek codecs are named after their product ID, e.g. 0892 -> ALC892
        devices.append((name, non_empty(pci_fields(pnp))))
    return devices


# Windows names that say nothing about the device (also in German, as Windows reports them in its display language)
GENERIC = re.compile(r"^(USB|HID)\b|Verbundgerät|Composite|Eingabegerät|Input Device|Audiogerät", re.I)


def usb_devices():
    """Connected USB devices, one per vendor/product ID (a device often registers several times)."""
    groups = {}
    for e in query("SELECT Name, Manufacturer, PNPDeviceID FROM Win32_PnPEntity WHERE PNPDeviceID LIKE 'USB%'"):
        ids = usb_ids(e["PNPDeviceID"])
        if ids:
            groups.setdefault(ids, []).append(e)
    devices = []
    for (vid, pid), entries in groups.items():
        names = [n for n in (clean(e["Name"]) for e in entries) if n and not GENERIC.search(n)]
        vendor = USB_VENDORS.get(vid) or next(
            (v for v in (clean(e["Manufacturer"]) for e in entries)
             if v and not v.startswith("(") and not v.startswith("Microsoft")), None)
        # Serial number: many devices have it as the last part of the ID (without "&", that would be a Windows number)
        serial = next((e["PNPDeviceID"].rsplit("\\", 1)[1] for e in entries
                       if "&MI_" not in e["PNPDeviceID"].upper() and "&" not in e["PNPDeviceID"].rsplit("\\", 1)[1]),
                      None)
        name = (names[0] if names else f"{t('hw.name.usb_device')} ({vendor})" if vendor
                else t("hw.name.usb_device"))
        devices.append((name, non_empty([
            ("vendor_id", with_name(vid, USB_VENDORS) if vid in USB_VENDORS
             else f"{vid} ({vendor})" if vendor else vid),
            ("product_id", pid),
            ("serial_number", serial),
            ("hardware_id", f"USB\\VID_{vid}&PID_{pid}"),
        ])))
    return sorted(devices, key=lambda d: d[0].lower())


# Category ID (name: hw.category.<ID> in i18n.json) -> function that reads it
CATEGORIES = (("processor", processors), ("graphics_card", graphics_cards), ("motherboard", motherboards),
              ("memory", memory_modules), ("drives", drives), ("network", network_adapters),
              ("audio", audio_devices), ("usb_devices", usb_devices))


def read_all():
    """All categories: [(category ID, [(device name, [(field ID, value), …]), …]), …] – empty categories are left out."""
    data = []
    for category, function in CATEGORIES:
        try:
            devices = function()
        except Exception:                        # one broken category shouldn't prevent the others
            logging.exception("Hardware data: category %s could not be read", category)
            continue
        if devices:
            data.append((category, devices))
    return data


# ---------- As text (copying in the tab Hardware data) ----------
def as_text(category, devices, private=False):
    """One category with all devices and fields – for pasting into a forum, chat, notes.
    private: privacy mode – sensitive fields as HIDDEN."""
    blocks = ["\n".join([name] + [f"  {field_name(field)}: {shown(field, value, private)}" for field, value in fields])
              for name, fields in devices]
    return f"=== {category_name(category)} ===\n" + "\n\n".join(blocks)


def all_as_text(data, private=False):
    """All categories, with computer name (HIDDEN in privacy mode) and date on top."""
    today = date.today()
    header = t("hw.text.header", pc=HIDDEN if private else platform.node(),
               date=fmt_date(today.year, today.month, today.day))
    return header + "\n\n" + "\n\n\n".join(as_text(c, d, private) for c, d in data) + "\n"


def model_names(devices):
    """The device names of a category – identical devices combined ("2× G.Skill …")."""
    names = Counter(name for name, _ in devices)
    return [f"{n}× {name}" if n > 1 else name for name, n in names.items()]


def model_names_as_text(data):
    """Only the model names: the category, below it one line per device with a tab in front."""
    lines = []
    for category, devices in data:
        lines += [category_name(category)] + [f"\t{name}" for name in model_names(devices)]
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    import sys
    sys.stdout.reconfigure(encoding="utf-8")
    for category, devices in read_all():
        print(f"\n=== {category_name(category)} ===")
        for name, fields in devices:
            print(f"  {name}")
            for field, value in fields:
                print(f"      {field_name(field):20} {value}")
