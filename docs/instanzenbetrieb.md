# Instanzenbetrieb

Mehrere StarCLX auf demselben Rechner, je eins pro Anlage.

## Was er kann

- Mehrere StarCLX gleichzeitig, eins je Anlage. Tray › „Weitere Anlage
  öffnen“ startet eine weitere Instanz mit eigenem Profil
  (`starclx --profile 2`, `3` …). Jedes Profil hat eigene Einstellungen,
  Konten, Chatverläufe, Protokolle und eigenen Webview-Speicher unter
  `profiles/<n>` in den üblichen Ordnern. Ein neues Profil übernimmt die
  Einstellungen (Geräte, Sprache …) des Standardprofils, aber keine Konten.
- Laufen zwei oder mehr, zeigen Fenstertitel, Schnellwahl und Tray-Tooltip
  die Anlage, z. B. „StarCLX – pbx.example.ch“.
- Jedes weitere Profil hat ein Symbol in eigener Farbe (Farbton gedreht),
  im Tray und an den Fenstern; das Standardprofil behält die Originalfarben.
  Unter Wayland nehmen GNOME und KDE das Symbol der Taskleiste aus der
  .desktop-Datei zur `app_id`. Weitere Profile melden sich deshalb als
  `starclx-p<n>` und legen beim Start einen im Menü versteckten Eintrag
  `~/.local/share/applications/starclx-p<n>.desktop` mit ihrem farbigen
  Symbol und dem Namen der Anlage an. Als Flatpak bestimmt die Sandbox die
  `app_id`; dort bleibt das Taskleisten-Symbol einheitlich.
- Oben im Tray-Menü steht die Anlage der Instanz.
- Der **Hauptclient** (zuerst gestartete Instanz) bekommt `tel:`-, `callto:`-
  und `sip:`-Links, Tastenkürzel und Login-Rücksprünge aus dem Browser, trägt
  Autostart und Link-Zuordnung ein und bedient Headset und Busylight für alle.
- Headset: Abheben nimmt den Anruf an, der zuerst klingelte, egal in welcher
  Instanz. Auflegen und Stumm wirken auf das laufende Gespräch. Klingel-,
  Gesprächs- und Stumm-LED zeigen den Zustand aller Instanzen.
- Bluetooth-Kopfhörer und -Headsets: Während eines Anrufs meldet sich der
  Hauptclient als Medienspieler (MPRIS); Play/Pause nimmt an bzw. legt auf.
- **Eine Instanz telefoniert, die anderen sind gesperrt**: Anrufe klingeln
  dort weiter, aber Annehmen und Wählen gehen erst nach dem Auflegen wieder.
  Ein Hinweis am Anruf nennt die Anlage, über die telefoniert wird.
- Busylight: blinkt, wenn es irgendwo klingelt, rot bei irgendeinem Gespräch,
  grün wenn alle frei sind.
- **Beenden** im Hauptclient fragt und beendet zuerst alle anderen, dann sich
  selbst. Beim nächsten Start öffnet der Hauptclient die zuletzt offenen
  Instanzen wieder. „Beenden“ in einer anderen Instanz beendet nur diese; sie
  startet dann nicht mehr mit.
- **Ausfall des Hauptclients** (Absturz, `kill`): Die älteste andere Instanz
  übernimmt sofort seine Aufgaben. Startet der alte Hauptclient neu, läuft er
  als normale Instanz weiter.

## Wie es funktioniert

`apps/desktop/src-tauri/src/instances.rs` ersetzt tauri-plugin-single-instance.
Jede Instanz belegt auf dem Session-Bus `<id>.Instance.<Profil>`
(`Default`, `P2` …) und stellt sich für `<id>.SingleInstance` an. Der Bus
gibt diesen Namen dem ersten; die anderen warten in seiner Warteschlange in
Startreihenfolge und bekommen ihn bei einem Ausfall automatisch. `<id>` ist
`ch.crazmoe.starface-linuxclient`, als Flatpak die Flatpak-ID.

Die anderen Instanzen melden dem Hauptclient ihren Anrufzustand über die
Methode `Report`; Headset-Tasten gibt er per `HeadsetAction` an die
betroffene Instanz zurück. Beginnt oder endet ein Gespräch, meldet die
Instanz das allen anderen per `Busy(Profil, Anlage, telefoniert)`.

## Testen

```sh
starclx &              # Hauptclient
starclx --profile 2 &  # zweite Anlage
busctl --user list | grep starface-linuxclient
```
