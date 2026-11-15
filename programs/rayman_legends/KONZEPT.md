# Konzept: Screen Zones

Stand: 2026-11-15. Nur Konzept, noch nichts davon ist gebaut.

Ein Programm, das beliebige Fenster rahmenlos in einen festen Bereich des Bildschirms setzt und dort hält.
Es ersetzt `rayman_borderless.ps1` und macht aus „ein Skript pro Spiel“ „ein Eintrag pro Spiel“.

## Ordner wird umbenannt

Dieser Ordner heißt noch `programs/rayman_legends/`. Als erster Schritt der Umsetzung benenne ich ihn in
`programs/screen_zones/` um. Jetzt noch nicht, weil sonst die funktionierende Rayman-Seite kaputt wäre.

Was beim Umbenennen mit angepasst werden muss:

| Stelle | Änderung |
|---|---|
| `program.json` | `id` wird `screen_zones`, `page` statt `run` |
| `data/rayman_legends.json` | wird einmalig nach `data/screen_zones.json` übernommen (Position Links/Mitte/Rechts bleibt erhalten) |
| `data/installed.json` | alter Eintrag `rayman_legends` raus, neuer rein |
| `README.md` Zeile 12, 47 | Tabelle und Ordnerbaum |
| `README.md` „Kleines Skript einbinden“ und `toolbox/script_page.py` (Kopfkommentar) | nennen Rayman als Vorbild für Skript-Programme. Das Vorbild fällt weg, die Texte brauchen ein neutrales Beispiel. `script_page.py` selbst bleibt. |
| `rayman_borderless.ps1` | wird gelöscht, sobald die Rust-Version dasselbe kann (Meilenstein 1) |

Der Ordner ist noch nicht in Git, es ist also ein einfaches Umbenennen ohne Verlauf.

Der Name ist ein Vorschlag. „Zonen“ beschreibt, was der Nutzer davon hat: Der eine breite Bildschirm bekommt wieder
Bereiche, die sich wie eigene Bildschirme verhalten.

## Ziel und Abgrenzung

**Ziel:** Ein Spiel einmal eintragen, danach landet es bei jedem Start von selbst rahmenlos in seiner Zone. Daneben
bleibt der Desktop sichtbar und benutzbar.

**Was es nicht kann und nicht versucht:**

- Echtes exklusives Vollbild umlenken. Das Spiel muss einen Fenstermodus haben. Spiele ohne Fenstermodus sind ein
  Fall für die Erweiterungen (Startparameter, siehe unten).
- Das Bild skalieren. Das Spiel rendert in der Größe seines Fensters. Skalieren ist eine Erweiterung.
- In fremde Prozesse eingreifen. Es werden nur die normalen Fensterfunktionen von Windows benutzt, nichts wird
  eingeschleust. Das ist dieselbe Technik wie Borderless Gaming und für Anti-Cheat unauffällig, eine Garantie gibt es
  bei Spielen mit Kernel-Anti-Cheat trotzdem nicht.

**Unterschied zu vorhandenen Tools:** FancyZones (PowerToys) ordnet Fenster in Zonen, nimmt ihnen aber nicht den
Rahmen und wendet nichts automatisch beim Programmstart an. Borderless Gaming macht rahmenlos mit fester Position,
kennt aber keine Zonen, kein Umschalten per Taste und lässt sich nicht erweitern.

## Aufbau

Zwei Teile, wie bei Hardware Monitor und Discord Plus:

```
Toolbox-Seite (Python, screen_zones_gui.py)        ScreenZones.exe (Rust, ohne Fenster)
  Zonen und Regeln bearbeiten            ──────►     liest data\screen_zones.json
  Status anzeigen                        ◄──────     meldet Status über lokalen Port
  Knöpfe: Spielen, Jetzt anwenden        ──────►     nimmt Befehle über lokalen Port
```

- **ScreenZones.exe** ist der Wächter. Er läuft unsichtbar, optional mit Autostart, und erledigt die ganze Arbeit.
  Er läuft auch ohne geöffnete Toolbox.
- **Die Toolbox-Seite** ist die Bedienung. Sie schreibt nur die Einstellungsdatei und spricht mit dem Wächter.

Warum die Bedienung nicht auch in Rust: Die Toolbox hat schon Seiten, Sprache, Einstellungen und Installieren.
Eine zweite Oberfläche in Rust wäre ein eigenes Fenster mit eigenem Aussehen neben der Toolbox. Falls du doch ein
eigenständiges Rust-Fenster willst, siehe „Offene Entscheidungen“.

### Ordner nach der Umsetzung

```
programs/screen_zones/
├─ program.json           id, page, starter, port, autostart
├─ i18n.json              Texte de/en
├─ screen_zones_gui.py    Toolbox-Seite
├─ README.md              Einrichtung, Bedienung, Technik
├─ ScreenZones.exe        fertig gebaut, liegt in Git (klonen + Setup = läuft, ohne Rust)
└─ rust/
   ├─ Cargo.toml
   └─ src/
      ├─ main.rs          Start, nur eine Instanz, Nachrichtenschleife
      ├─ config.rs        Einstellungsdatei lesen, prüfen, neu laden
      ├─ zones.rs         Zone → Pixel-Rechteck (reine Rechnung, gut testbar)
      ├─ rules.rs         Fenster ↔ Regel zuordnen (reine Logik, gut testbar)
      ├─ windows.rs       alle Win32-Aufrufe: Rahmen, Position, Zustand merken
      ├─ watcher.rs       Fenster-Ereignisse von Windows
      ├─ hotkeys.rs       globale Tasten
      └─ ipc.rs           lokaler Port für die Toolbox-Seite
```

`rust/target/` kommt in `.gitignore`. Rust ist auf diesem PC vorhanden (cargo 1.99, MSVC). Die `.exe` in Git zu
legen passt zum Projektziel „portabel“: Wer klont, braucht kein Rust.

### Rust-Bausteine

- `windows` (offizielle Win32-Anbindung von Microsoft) für alle Fensterfunktionen
- `serde` und `serde_json` für die Einstellungsdatei
- sonst nichts. Kein GUI-Framework, kein async. Eine Nachrichtenschleife in einem Thread reicht.
- `#![windows_subsystem = "windows"]`, damit nie eine Konsole aufgeht
- Manifest mit „Per-Monitor-V2“-DPI, damit immer in echten Pixeln gerechnet wird (das Skript macht das heute per
  `SetProcessDpiAwarenessContext(-4)`)

## Begriffe

**Zone:** ein benannter Bereich auf einem Bildschirm. Nicht in festen Pixeln gespeichert, sondern als Beschreibung,
damit sie eine andere Auflösung übersteht:

- Seitenverhältnis (z. B. 16:9) plus Anker (links, Mitte, rechts), volle Höhe. Auf 5120×1440 ergibt das 2560×1440.
- oder Anteil der Breite (Hälfte, Drittel, zwei Drittel) plus Anker
- oder feste Pixel (x, y, Breite, Höhe) für Sonderfälle

Mitgelieferte Zonen: 16:9 links / Mitte / rechts, 21:9 Mitte, linke Hälfte, rechte Hälfte, ganzer Bildschirm.

**Regel:** „Dieses Programm gehört in diese Zone“. Eine Regel besteht aus:

- Erkennung: Name der `.exe` (Pflicht), optional voller Pfad, Fenstertitel (Teiltext), Fensterklasse
- Zone
- Schaltern: rahmenlos, über der Taskleiste solange aktiv, Maus einsperren, Rahmen bewachen
- optional Startziel (`uplay://launch/410/0`, `steam://rungameid/…`, eine `.exe`) für den Knopf „Spielen“
- optional Wartezeit nach dem Erscheinen des Fensters (Rayman braucht heute 3 Sekunden)

## Was der Wächter tut

### Fenster finden

Windows meldet Fenster-Ereignisse (`SetWinEventHook`: Fenster erscheint, wird aktiv, wird verschoben, verschwindet).
Der Wächter reagiert darauf und fragt nicht ständig nach. Im Leerlauf verbraucht er dadurch praktisch keine
Rechenzeit. Das heutige Skript fragt alle 300 ms.

Welches Fenster eines Programms das richtige ist: sichtbar, ohne Besitzerfenster, kein Werkzeugfenster, größer als
eine Mindestgröße. Das filtert Startbildschirme, Launcher-Fenster und unsichtbare Hilfsfenster heraus.

Beim Start des Wächters werden alle schon offenen Fenster einmal geprüft. Reihenfolge „erst Spiel, dann Wächter“
geht also genauso wie umgekehrt.

### Fenster setzen

1. Ursprünglichen Stil und Position merken (zum Zurücksetzen).
2. Rahmen entfernen: `WS_CAPTION`, `WS_THICKFRAME`, `WS_SYSMENU`, Min-/Max-Knöpfe, dazu die erweiterten Randstile.
3. Auf das Rechteck der Zone setzen (`SetWindowPos` mit `SWP_FRAMECHANGED`).
4. Nachprüfen, ob der Inhaltsbereich wirklich so groß ist wie die Zone. Wenn nicht: bis zu dreimal wiederholen, dann
   Status „Spiel nimmt die Größe nicht an“.

### Fenster halten

- **Rahmen bewachen:** Bringt das Spiel den Rahmen zurück (Rayman nach Optionsänderung), wird er wieder entfernt.
  Auslöser ist das Verschieben-/Größen-Ereignis, kein Zeittakt.
- **Über der Taskleiste:** Ein Fenster, das nur einen Teil des Bildschirms füllt, gilt für Windows nicht als
  Vollbild, die Taskleiste bleibt also davor. Deshalb „immer im Vordergrund“, aber nur solange das Fenster aktiv
  ist. Sobald du in ein anderes Fenster klickst, wird das zurückgenommen. So macht es das Skript heute auch.
- **Maus einsperren (Schalter, Standard aus):** Solange das Spiel aktiv ist, bleibt der Mauszeiger in der Zone
  (`ClipCursor`). Wichtig für Spiele, die am Bildschirmrand scrollen. Alt+Tab gibt die Maus frei.

### Zurücksetzen

Wird eine Regel gelöscht, ausgeschaltet oder der Wächter beendet, bekommen alle betroffenen Fenster ihren alten
Stil und ihre alte Position zurück. Kein Fenster bleibt ohne Rahmen hängen.

### Sonderfälle

| Fall | Verhalten |
|---|---|
| Spiel läuft als Administrator | Windows verbietet den Zugriff von einem normalen Programm. Der Wächter erkennt das und meldet es. Lösung: Wächter über eine Aufgabe mit Adminrechten starten (so macht es Hardware Monitor mit `task`). |
| Spiel ist im exklusiven Vollbild | Erkennbar daran, dass das Fenster den ganzen Bildschirm füllt und sich nicht verschieben lässt. Status: „Im Spiel Fenstermodus einstellen“. |
| Spiel hat mehrere Fenster oder startet über einen Launcher | Erkennung über Mindestgröße, notfalls Fenstertitel in der Regel |
| Spiel erstellt sein Fenster neu (Wechsel der Auflösung) | neues Fenster löst das Ereignis erneut aus, Regel greift wieder |
| Auflösung oder Bildschirm ändert sich | Zonen werden neu berechnet, Fenster neu gesetzt |
| Fenster minimiert | wird in Ruhe gelassen, beim Wiederherstellen neu gesetzt |
| Zwei Regeln passen | die genauere gewinnt (Pfad vor Name, mit Titel vor ohne) |
| Einstellungsdatei kaputt oder halb geschrieben | alte Einstellungen bleiben aktiv, Fehler ins Log, Seite zeigt ihn |
| Wächter stürzt ab | Fenster bleiben, wie sie sind. Beim nächsten Start werden sie wieder übernommen. Zurücksetzen geht dann nur über „Rahmen zurückgeben“ auf der Seite oder Neustart des Spiels. |
| Wächter zweimal gestartet | benannter Mutex, die zweite Instanz beendet sich sofort |

## Einstellungsdatei

`data/screen_zones.json`, geschrieben von der Toolbox-Seite, gelesen vom Wächter. Die Seite schickt nach dem
Speichern „neu laden“ über den Port, der Wächter muss die Datei also nicht beobachten.

```json
{
  "version": 1,
  "enabled": true,
  "zones": [
    { "id": "left-16-9",   "aspect": "16:9", "anchor": "left" },
    { "id": "center-16-9", "aspect": "16:9", "anchor": "center" },
    { "id": "right-16-9",  "aspect": "16:9", "anchor": "right" }
  ],
  "rules": [
    {
      "name": "Rayman Legends",
      "exe": "Rayman Legends.exe",
      "zone": "center-16-9",
      "borderless": true,
      "topmost_while_active": true,
      "guard_frame": true,
      "clip_cursor": false,
      "delay_ms": 3000,
      "launch": "uplay://launch/410/0",
      "enabled": true
    }
  ],
  "hotkeys": {
    "zone_left": "Ctrl+Alt+Left",
    "zone_center": "Ctrl+Alt+Down",
    "zone_right": "Ctrl+Alt+Right",
    "restore": "Ctrl+Alt+Up"
  }
}
```

`version` erlaubt spätere Änderungen am Format, ohne alte Dateien zu verlieren. Unbekannte Felder werden ignoriert.

## Verbindung Seite ↔ Wächter

Lokaler Port wie bei den anderen Programmen (`"port"` in `program.json`, Vorschlag 47818, nur `127.0.0.1`).
Wie Hardware Monitor und Discord Plus ihren Port genau benutzen, muss ich vor der Umsetzung nachlesen und dann
gleich machen. Gebraucht werden diese Befehle:

- `status` → läuft, welche Fenster gerade gehalten werden, Fehler je Regel
- `reload` → Einstellungen neu lesen
- `windows` → Liste der offenen Fenster (für „Programm hinzufügen“)
- `apply` / `restore` → Regel sofort anwenden bzw. Fenster zurücksetzen
- `quit` → sauber beenden, vorher alles zurücksetzen

## Bedienung

Alles über die Toolbox-Seite oder die `.exe`, nichts über Befehlszeilen-Schalter.

### Die Seite

```
┌ Screen Zones ─────────────────────────────────────────────┐
│ Wächter: läuft ●      [Beenden]   ☑ Mit Windows starten   │
│                                                           │
│ Spiele                                 [+ Hinzufügen]     │
│ ┌───────────────────────────────────────────────────────┐ │
│ │ Rayman Legends     [Links|Mitte|Rechts]  ● gehalten   │ │
│ │                    [Spielen]  [⚙]  [Entfernen]        │ │
│ ├───────────────────────────────────────────────────────┤ │
│ │ Anderes Spiel      [Links|Mitte|Rechts]  ○ läuft nicht│ │
│ └───────────────────────────────────────────────────────┘ │
│                                                           │
│ Tasten      Aktives Fenster nach links / Mitte / rechts   │
│ Anleitung   Was im Spiel einzustellen ist                 │
└───────────────────────────────────────────────────────────┘
```

- **Zeile je Spiel:** Name, Zonen-Auswahl, Status, „Spielen“ (nur wenn ein Startziel eingetragen ist). Ein Wechsel
  der Zone verschiebt das laufende Spiel sofort, wie heute bei Rayman.
- **Status je Spiel in Klartext:** läuft nicht / gehalten / wartet auf Fenster / braucht Adminrechte / Spiel ist im
  Vollbild / Spiel nimmt die Größe nicht an. Jeder Fehlerstatus sagt dazu, was zu tun ist.
- **⚙ je Spiel:** die Schalter, Wartezeit, Startziel, Erkennung über Titel. Standardwerte passen für die meisten
  Spiele, niemand muss hier hinein.
- **Anleitung auf der Seite**, nicht nur in der README: Spiel auf Fenstermodus stellen, Auflösung auf die der Zone,
  bei Adminrechten der Hinweis auf die Aufgabe.

### Programm hinzufügen

„Aktives Fenster hinzufügen“ als Knopf geht nicht direkt, denn beim Klick ist die Toolbox selbst das aktive
Fenster. Deshalb zwei Wege:

1. **Liste:** „Hinzufügen“ zeigt alle offenen Fenster mit Titel und Programmname. Eines anklicken, Zone wählen,
   fertig. Das ist der Hauptweg.
2. **Taste:** Im Spiel eine Zonentaste drücken. Das Fenster wird sofort gesetzt, und auf der Seite erscheint danach
   „Regel für dieses Programm speichern?“.

### Tasten

Global, frei belegbar, abschaltbar. Sie wirken auf das gerade aktive Fenster, auch ohne Regel:

- nach links / Mitte / rechts
- Rahmen zurückgeben

Ist eine Taste schon von einem anderen Programm belegt, meldet die Seite das, statt still nichts zu tun.

### Verhalten nebenher

- Der Wächter öffnet nie ein Fenster und nimmt nie den Fokus weg.
- Kein Symbol in der Taskleiste nötig. Ob ein Symbol im Infobereich sinnvoll ist, steht unter „Offene Entscheidungen“.
- Fehler stehen in `data/logs/` und als Status auf der Seite.

## Einbindung in die Toolbox

```json
{
  "id": "screen_zones",
  "name": "Screen Zones",
  "version": "1.0",
  "category": "General",
  "description": "Puts games and programs borderless into a fixed area of an ultrawide screen (screen_zones).",
  "page": "screen_zones_gui.py",
  "starter": "ScreenZones.exe",
  "port": 47818,
  "autostart": "WindowsToolbox ScreenZones"
}
```

- Auf der Seite `PROGRAMM = Programm(laeuft, beenden, starten)`, damit „Toolbox und alle Programme“ den Wächter
  mit beendet und startet.
- `starter` bezeichnet bei den anderen Programmen eine kleine C#-Starter-`.exe` für ein Python-Skript. Hier ist die
  `.exe` das Programm selbst. Ob das Setup damit zurechtkommt oder einen eigenen Schlüssel braucht, muss ich in
  `setup/` prüfen.
- Kategorie wechselt von „Games“ zu „General“, weil es nicht mehr nur ein Spiel betrifft.

## Tests

- **Rechnung und Zuordnung** (`zones.rs`, `rules.rs`) sind reine Funktionen: normale Rust-Tests mit `cargo test`.
  Beispiele: 16:9 mittig auf 5120×1440 ergibt x=1280, Breite 2560. Pfad-Regel schlägt Namens-Regel.
- **Fensterfunktionen:** Ein Testprogramm erzeugt ein eigenes Fenster außerhalb des sichtbaren Bereichs, der Wächter
  setzt es, der Test liest Stil und Rechteck zurück. So poppt beim Testen nichts auf.
- **Echter Test:** Rayman Legends, alle drei Positionen, Optionsmenü öffnen (Rahmen kommt zurück), Alt+Tab,
  Wächter beenden (Rahmen muss wiederkommen). Diesen Teil kannst nur du am Spiel bestätigen.

## Meilensteine

1. **Gleichstand mit dem Skript.** Ordner umbenennen, Wächter mit einer Regel, Zonen links/Mitte/rechts,
   Rahmen bewachen, Taskleiste. Seite mit einer Zeile für Rayman. Danach `rayman_borderless.ps1` löschen.
2. **Mehrere Spiele.** Liste, Hinzufügen über die Fensterliste, Status je Spiel, Zurücksetzen, Autostart.
3. **Tasten** und „Regel speichern?“ nach Tastendruck.
4. **Sonderfälle:** Adminrechte über Aufgabe, Maus einsperren, eigene Zonen.

## Mögliche Erweiterungen

- **Schwarzer Hintergrund:** ein schwarzes Fenster hinter dem Spiel über den Rest des Bildschirms, ein- und
  ausblendbar per Taste. Das ist der „Kino-Modus“, den die NVIDIA-Lösung bietet, nur umschaltbar.
- **Startparameter je Spiel:** viele Spiele ohne Fenstermodus im Menü kennen `-windowed` oder `-noborder`. Die
  Regel startet das Spiel dann gleich richtig.
- **Skalieren:** Spiel rendert kleiner (z. B. 1920×1080), wird auf die Zone vergrößert. Geht über die
  Windows-Lupen-Schnittstelle oder einen eigenen Bildabgriff. Deutlich aufwendiger.
- **Zwei Spiele oder Spiel plus Video nebeneinander:** linke und rechte Zone gleichzeitig belegt, eine Taste
  tauscht die Seiten.
- **Zonen-Editor:** Zonen mit der Maus auf einem Bild des Bildschirms aufziehen.
- **Profile:** „Spielen“, „Arbeiten“, „Stream“ mit je eigenen Regeln, umschaltbar per Taste.
- **Normale Programme:** Browser, Discord, OBS in Zonen. Technisch dasselbe, braucht nur „rahmenlos“ aus.
- **Vorlagen:** bekannte Spiele mit passenden Einstellungen (Wartezeit, Titel) zum Anklicken.
- **Ton:** Spiel stumm schalten, wenn es nicht aktiv ist.
- **Mehrere Bildschirme:** Zonen gehören schon im Format zu einem Bildschirm, die Oberfläche dafür fehlt noch.
- **Symbol im Infobereich** mit Schnellwechsel der Zone.

## Offene Entscheidungen

1. **Name.** „Screen Zones“ / `screen_zones` ist mein Vorschlag.
2. **Oberfläche.** Empfehlung: Toolbox-Seite in Python, Wächter in Rust. Alternative: alles in Rust mit eigenem
   Fenster (`egui`), dann wäre es auch ohne Toolbox benutzbar, sähe aber anders aus als der Rest und die Seite wäre
   nur noch ein Startknopf.
3. **Standard-Tasten.** Strg+Alt+Pfeile sind bei manchen Grafiktreibern mit „Bildschirm drehen“ belegt. Alternative:
   Win+Alt+Pfeile.
4. **`.exe` in Git** (Empfehlung, portabel) oder beim Setup bauen (braucht Rust auf jedem PC).
5. **Symbol im Infobereich** ja oder nein. Empfehlung: erst einmal nein, die Seite reicht.
