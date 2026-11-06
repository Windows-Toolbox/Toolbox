"""
What the detail window of the Hardware Monitor shows – tab by tab, one row per value.

  Add a value:     write a row Value("Name") into the matching group
  Leave one out:   delete the row
  Order:           as written here

The exact sensor names are in the output of `python hardware_monitor.py --list`
(there also in square brackets the device for Group(...): cpu, gpu, ram, motherboard, drive, network, battery).

  Group("drive", ...)                      applies to every drive separately
  Group("gpu", ..., name="Intel*")         only devices whose name matches
  Value("Fan #*")                          * stands for any text – here: all fans
  Value("GPU Core", type="Clock")          type only needed if the name exists several times
  Value("Package", text="CPU total")       own label (the same in every language – or an ID from i18n.json)
  Value("CPU Core #*", exclude="*Distance*")   wildcard, but without the names that match this
  Section("Fans", Value(...), Value(...))  collapsible sub-item in a group
  Section(..., one_row=True)               values with the same name in one row (Fan #1: speed, then % as bar)
  HardwareData()                           instead of groups: fixed data like vendor and product IDs
  PowerConsumption()                       instead of groups: power consumption with a chart
  FanControl()                             instead of groups: fan curves (graphics card and case fans)
  DriveHealth()                            instead of groups: drive health like CrystalDiskInfo

The names differ depending on the hardware (AMD: "Core (Tctl/Tdie)", Intel: "CPU Package"). What doesn't exist on
a PC is simply not shown – that's why the names for both are listed here side by side. Tabs whose devices the PC
doesn't have at all (e.g. motherboard sensors on a laptop, battery on a desktop) are hidden.

Labels – tab titles, Section, title=, text= – and the explanations below are IDs from i18n.json (e.g. "tab.gpu");
their text is there for every language. A custom text without ID works too; it then reads the same in every
language.

Saved? Close the window and open it again – the changes are there right away.
"""

from detail_window import (AllSensors, DriveHealth, FanControl, Group, HardwareData,  # noqa: F401
                           PowerConsumption, Section, Tab, Value)       # (AllSensors see below)

# The fans – one row per fan: speed, then the control as a bar. Each is in two tabs.
# Motherboards name their headers differently ("Fan #1", "CPU Fan", "Chassis Fan #2", "Pump" …).
FANS = Section("section.fans",
               Value("*Fan*", type="Fan"),
               Value("*Pump*", type="Fan"),
               Value("*Fan*", type="Control"),
               Value("*Pump*", type="Control"),
               one_row=True)
GPU_FANS = Section("section.fans",
                   Value("GPU Fan*", type="Fan"),
                   Value("GPU Fan*", type="Control"),
                   one_row=True)

TABS = [
    Tab("tab.temperatures",
        Group("cpu",
              Value("Core (Tctl/Tdie)"),                # AMD
              Value("CCD* (Tdie)"),
              Value("CPU Package", type="Temperature"),  # Intel
              Value("Core Max", type="Temperature")),
        Group("gpu",
              Value("GPU Core", type="Temperature"),
              Value("GPU Hot Spot"),
              Value("GPU Memory Junction"),
              GPU_FANS),
        Group("motherboard",
              FANS)),

    Tab("tab.cpu",
        Group("cpu",
              Value("CPU Total"),
              Value("CPU Core Max"),
              Value("Package", type="Power"),           # AMD
              Value("CPU Package", type="Power"),       # Intel
              Value("CPU Cores", type="Power"),
              Value("Cores (Average)"),
              Value("Cores (Average Effective)"),
              Value("CPU Core #*", type="Clock"),       # Intel: clock per core (without average)
              Value("Core (Tctl/Tdie)"),
              Value("CCD* (Tdie)"),
              Value("CPU Package", type="Temperature"),
              Value("Core Max", type="Temperature"),
              Value("CPU Core #*", type="Temperature", exclude="*Distance to TjMax"),
              Value("Core (SVI2 TFN)"),
              Value("SoC (SVI2 TFN)"),
              Value("CPU Core", type="Voltage"),
              Value("Bus Speed"))),

    Tab("tab.gpu",
        Group("gpu",
              Value("GPU Core", type="Temperature"),
              Value("GPU Hot Spot"),
              Value("GPU Core", type="Load", text="value.gpu_core_load"),
              Value("GPU Memory Controller"),
              Value("GPU Video Engine"),
              Value("GPU Bus"),
              Value("GPU Power", type="Load"),
              Value("GPU Board Power"),
              Value("GPU Package"),
              Value("GPU Core", type="Clock", text="value.gpu_core_clock"),
              Value("GPU Memory", type="Clock", text="value.gpu_memory_clock"),
              Value("GPU Core Voltage"),
              Value("GPU Memory Used"),
              Value("GPU Memory Free"),
              Value("GPU Memory Total"),
              Value("GPU PCIe Rx"),
              Value("GPU PCIe Tx"),
              GPU_FANS),
        Group("gpu",                                    # Intel graphics: only delivers power and Windows values
              Value("GPU Power", type="Power"),
              Value("D3D 3D"),
              Value("D3D Dedicated Memory Used"),
              Value("D3D Shared Memory Used"),
              Value("D3D Shared Memory Free"),
              Value("D3D Shared Memory Total"),
              name="Intel*")),

    Tab("tab.ram",
        Group("ram",
              Value("Memory", type="Load", text="value.load"),
              Value("Memory Used"),
              Value("Memory Available"),
              name="Total Memory", title="title.memory"),
        Group("ram",
              Value("Memory", type="Load", text="value.load"),
              Value("Memory Used"),
              Value("Memory Available"),
              name="Virtual Memory", title="title.with_page_file"),
        Group("ram",                                    # every module separately
              Value("DIMM #*"),
              Value("Capacity"))),

    Tab("tab.motherboard",
        Group("motherboard",
              Value("CPU", type="Temperature"),
              Value("Motherboard"),
              Value("VRM"),
              Value("Auxiliary*"),
              Value("Vcore"),
              Value("+12V"),
              Value("+5V"),
              Value("+3.3V"),
              Value("+3V Standby"),
              Value("+1.8V"),
              Value("Chipset 1.05V"),
              FANS)),

    Tab("tab.drives",
        Group("drive",                                  # every drive separately
              Value("Composite Temperature"),
              Value("Temperature"),
              Value("Temperature #*"),
              Value("Warning Temperature"),
              Value("Critical Temperature"),
              Value("Used Space"),
              Value("Free Space"),
              Value("Total Space"),
              Value("Read Rate"),
              Value("Write Rate"),
              Value("Total Activity"),
              Value("Life"),
              Value("Percentage Used"),
              Value("Available Spare"),
              Value("Available Spare Threshold"),
              Value("Data Read"),
              Value("Data Written"),
              Value("Power On Count"),
              Value("Power On Hours"))),

    Tab("tab.network",
        Group("network",
              Value("Download Speed"),
              Value("Upload Speed"),
              Value("Network Utilization"),
              Value("Data Downloaded"),
              Value("Data Uploaded"))),

    Tab("tab.battery",
        Group("battery",
              Value("Charge Level"),
              Value("Remaining Capacity"),
              Value("Remaining Time*"),
              Value("*Rate", type="Power"),             # Discharge Rate or Charge Rate
              Value("*Current"),
              Value("Voltage"),
              Value("Fully-Charged Capacity"),
              Value("Designed Capacity"),
              Value("Degradation Level"),
              Value("Cycle Count"))),

    # Power consumption of the whole PC: now, today, this month and a chart (see energy.py, energy_view.py)
    Tab("tab.power", PowerConsumption()),

    # Fan curves for the graphics card fans and the motherboard fan headers (see fans.py, fan_view.py)
    Tab("tab.fans", FanControl()),

    # Health of the drives like CrystalDiskInfo: status, temperature, life, SMART table (see drive_view.py)
    Tab("tab.health", DriveHealth()),

    # Fixed data instead of measurements: manufacturer, model, IDs, serial numbers (see hardware_data.py)
    Tab("tab.hardware_data", HardwareData()),

    # Everything LibreHardwareMonitor finds, for looking up sensor names – comment it back in if needed:
    # Tab("tab.all", AllSensors()),
]


# ---------- Explanations in the footer ----------
# (sensor type, pattern, text ID) – the pattern has to match the whole sensor name (regular expression).
# The texts are in i18n.json under the ID ("explain.…") – there for every language.
# {0} in the text is replaced by the first group in parentheses of the pattern (e.g. the number in "CCD1").
# The sensor type matters: "GPU Core" is e.g. a temperature as well as a load and a clock.
EXPLANATIONS = [
    # --- Temperatures: CPU ---
    ("Temperature", r"Core \(Tctl/Tdie\)", "explain.temp.tctl_tdie"),
    ("Temperature", r"Core \(Tctl\)", "explain.temp.tctl"),
    ("Temperature", r"Core \(Tdie\)", "explain.temp.tdie"),
    ("Temperature", r"CCD(\d+) \(Tdie\)", "explain.temp.ccd"),
    ("Temperature", r"CCDs Max \(Tdie\)", "explain.temp.ccds_max"),
    ("Temperature", r"CCDs Average \(Tdie\)", "explain.temp.ccds_average"),
    ("Temperature", r"SoC", "explain.temp.soc"),
    ("Temperature", r"CPU Package", "explain.temp.cpu_package"),
    ("Temperature", r"Core Max", "explain.temp.core_max"),
    ("Temperature", r"Core Average", "explain.temp.core_average"),
    ("Temperature", r"CPU Core #(\d+)", "explain.temp.cpu_core"),
    ("Temperature", r"(?:.* )?Distance to TjMax", "explain.temp.distance_tjmax"),
    # --- Temperatures: GPU ---
    ("Temperature", r"GPU Core", "explain.temp.gpu_core"),
    ("Temperature", r"GPU Hot Spot", "explain.temp.gpu_hot_spot"),
    ("Temperature", r"GPU Memory Junction", "explain.temp.gpu_memory_junction"),
    ("Temperature", r"GPU Memory", "explain.temp.gpu_memory"),
    ("Temperature", r"GPU VR .+", "explain.temp.gpu_vr"),
    ("Temperature", r"GPU Liquid", "explain.temp.gpu_liquid"),
    # --- Temperatures: motherboard / drives ---
    ("Temperature", r"CPU", "explain.temp.motherboard_cpu"),
    ("Temperature", r"System|Motherboard", "explain.temp.system"),
    ("Temperature", r"PCH|Chipset", "explain.temp.chipset"),
    ("Temperature", r"VRM.*", "explain.temp.vrm"),
    ("Temperature", r"Temperature #(\d+)", "explain.temp.unnamed"),
    ("Temperature", r"Temperature( \d+)?", "explain.temp.drive"),
    ("Temperature", r"Composite Temperature", "explain.temp.composite"),
    ("Temperature", r"Warning Temperature", "explain.temp.warning"),
    ("Temperature", r"Critical Temperature", "explain.temp.critical"),
    # --- Load ---
    ("Load", r"CPU Total", "explain.load.cpu_total"),
    ("Load", r"CPU Core Max", "explain.load.cpu_core_max"),
    ("Load", r"CPU Core #(\d+)(?: Thread #(\d+))?", "explain.load.cpu_core"),
    ("Load", r"GPU Core", "explain.load.gpu_core"),
    ("Load", r"GPU Memory Controller", "explain.load.gpu_memory_controller"),
    ("Load", r"GPU Video Engine", "explain.load.gpu_video_engine"),
    ("Load", r"GPU Bus", "explain.load.gpu_bus"),
    ("Load", r"GPU Memory", "explain.load.gpu_memory"),
    ("Load", r"D3D .+", "explain.load.d3d"),
    ("Load", r"Memory", "explain.load.memory"),
    ("Load", r"Virtual Memory", "explain.load.virtual_memory"),
    ("Load", r"Used Space", "explain.load.used_space"),
    ("Load", r"(Total|Read|Write) Activity", "explain.load.activity"),
    ("Load", r"Network Utilization", "explain.load.network"),
    # --- Clock ---
    ("Clock", r"(?:CPU )?Core #(\d+)", "explain.clock.cpu_core"),
    ("Clock", r"Bus Speed", "explain.clock.bus"),
    ("Clock", r"Cores \(Average.*\)", "explain.clock.cores_average"),
    ("Clock", r"GPU Core", "explain.clock.gpu_core"),
    ("Clock", r"GPU Memory", "explain.clock.gpu_memory"),
    ("Clock", r"GPU Shader", "explain.clock.gpu_shader"),
    # --- Power ---
    ("Power", r"(CPU )?Package", "explain.power.package"),
    ("Power", r"Core #(\d+) \(SMU\)", "explain.power.core_smu"),
    ("Power", r"(?:CPU )?Cores", "explain.power.cores"),
    ("Power", r"SoC", "explain.power.soc"),
    ("Power", r"CPU Memory", "explain.power.cpu_memory"),
    ("Power", r"CPU Platform", "explain.power.platform"),
    ("Power", r"GPU (Package|Power)", "explain.power.gpu"),
    ("Power", r"Discharge Rate", "explain.power.discharging"),
    ("Power", r"Charge Rate", "explain.power.charging"),
    # --- Voltage ---
    ("Voltage", r"Core \(SVI\d? ?TFN\)", "explain.voltage.core_svi"),
    ("Voltage", r"SoC \(SVI\d? ?TFN\)", "explain.voltage.soc_svi"),
    ("Voltage", r"Vcore|CPU VCore", "explain.voltage.vcore"),
    ("Voltage", r"CPU Core", "explain.voltage.cpu_core"),
    ("Voltage", r"CPU Core #(\d+)", "explain.voltage.cpu_core_nr"),
    ("Voltage", r"\+?(12|5|3[.,]3)V", "explain.voltage.psu_rail"),
    ("Voltage", r"VBat|Vbat|CMOS Battery|Battery", "explain.voltage.cmos"),
    ("Voltage", r"GPU Core", "explain.voltage.gpu_core"),
    ("Voltage", r"Voltage", "explain.voltage.battery"),
    # --- Battery ---
    ("Current", r"Discharge Current", "explain.battery.discharge_current"),
    ("Current", r"Charge Current", "explain.battery.charge_current"),
    ("Energy", r"Remaining Capacity", "explain.battery.remaining"),
    ("Energy", r"Fully-Charged Capacity", "explain.battery.full_charge"),
    ("Energy", r"Designed Capacity", "explain.battery.design"),
    ("TimeSpan", r"Remaining Time.*", "explain.battery.remaining_time"),
    # --- Fans ---
    ("Fan", r"GPU Fan( \d+)?", "explain.fan.gpu"),
    ("Fan", r"Fan #(\d+)", "explain.fan.motherboard"),
    ("Fan", r"Pump.*", "explain.fan.pump"),
    ("Control", r"GPU Fan( \d+)?", "explain.fan.gpu_control"),
    ("Control", r"Fan( Control)? #(\d+)", "explain.fan.motherboard_control"),
    # --- Memory / data ---
    ("Data", r"Memory Used", "explain.data.memory_used"),
    ("Data", r"Memory Available", "explain.data.memory_available"),
    ("Data", r"Virtual Memory Used", "explain.data.virtual_used"),
    ("Data", r"Virtual Memory Available", "explain.data.virtual_available"),
    ("Data", r"Data Read", "explain.data.read"),
    ("Data", r"Data Written", "explain.data.written"),
    ("Data", r"Data Uploaded", "explain.data.uploaded"),
    ("Data", r"Data Downloaded", "explain.data.downloaded"),
    ("SmallData", r"GPU Memory Used", "explain.data.gpu_used"),
    ("SmallData", r"GPU Memory Free", "explain.data.gpu_free"),
    ("SmallData", r"GPU Memory Total", "explain.data.gpu_total"),
    ("SmallData", r"D3D Dedicated Memory Used", "explain.data.d3d_dedicated"),
    ("SmallData", r"D3D Shared Memory Used", "explain.data.d3d_shared_used"),
    ("SmallData", r"D3D Shared Memory Free", "explain.data.d3d_shared_free"),
    ("SmallData", r"D3D Shared Memory Total", "explain.data.d3d_shared_total"),
    # --- Throughput ---
    ("Throughput", r"Read Rate", "explain.throughput.read"),
    ("Throughput", r"Write Rate", "explain.throughput.write"),
    ("Throughput", r"Upload Speed", "explain.throughput.upload"),
    ("Throughput", r"Download Speed", "explain.throughput.download"),
    ("Throughput", r"GPU PCIe Rx", "explain.throughput.pcie_rx"),
    ("Throughput", r"GPU PCIe Tx", "explain.throughput.pcie_tx"),
    # --- Level ---
    ("Level", r"Available Spare", "explain.level.spare"),
    ("Level", r"Available Spare Threshold", "explain.level.spare_threshold"),
    ("Level", r"Life", "explain.level.life"),
    ("Level", r"Percentage Used", "explain.level.used"),
    ("Level", r"Charge Level", "explain.level.charge_level"),
    ("Level", r"Degradation Level", "explain.level.wear"),
    # --- Counters ---
    ("Factor", r"Power On Count", "explain.counter.power_on_count"),
    ("Factor", r"Power On Hours", "explain.counter.power_on_hours"),
]

TYPE_EXPLANATIONS = {   # fallback if no more specific entry above matches
    "Temperature": "explain.type.Temperature",
    "Load": "explain.type.Load",
    "Clock": "explain.type.Clock",
    "Power": "explain.type.Power",
    "Voltage": "explain.type.Voltage",
    "Current": "explain.type.Current",
    "Energy": "explain.type.Energy",
    "TimeSpan": "explain.type.TimeSpan",
    "Fan": "explain.type.Fan",
    "Control": "explain.type.Control",
    "Data": "explain.type.Data",
    "SmallData": "explain.type.SmallData",
    "Throughput": "explain.type.Throughput",
    "Level": "explain.type.Level",
}
