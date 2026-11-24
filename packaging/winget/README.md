# winget

Eintrag im [Windows Package Manager Community Repository](https://github.com/microsoft/winget-pkgs),
Paket-ID `crazmoe.StarCLX`. Danach installiert man StarCLX unter Windows mit

```
winget install crazmoe.StarCLX
```

Bis Version 2.0 hiess die Windows-Version StarCMX (`crazmoe.StarCMX`). Dieser Eintrag ist in
winget-pkgs aufgenommen, wird aber nicht weitergeführt: StarCLX bekommt die neue ID
`crazmoe.StarCLX`. Das StarCLX-Setup entfernt ein installiertes StarCMX selbst und übernimmt
Einstellungen und Anmeldung. Der alte Eintrag lässt sich danach entfernen (siehe unten).

## Erste Einreichung (einmalig, von Hand)

`manifests/` hat denselben Aufbau wie in winget-pkgs; der Ordner
`manifests/c/crazmoe/StarCLX/2.1.0/` wird dort 1:1 eingereicht, am einfachsten unter Windows mit

```
winget install wingetcreate
wingetcreate submit --token <token> packaging\winget\manifests\c\crazmoe\StarCLX\2.1.0
```

Alternativ im Fork `crazmoe/winget-pkgs` einen Branch anlegen, den Ordner an dieselbe Stelle
hochladen und einen PR gegen `microsoft/winget-pkgs` öffnen. Die Prüfung von Microsoft
(Installation in einer Test-VM, Virenscan) dauert meist ein bis drei Tage.

Vor dem Einreichen lokal prüfen:

```
winget validate --manifest <ordner>
winget install --manifest <ordner>
```

## Updates (automatisch)

Der Job `winget` in `.github/workflows/ci.yml` meldet jedes neue Release über
[winget-releaser](https://github.com/vedantmgoyal9/winget-releaser) an winget: Er übernimmt die
Angaben der letzten Version, setzt Version, URL und Hash neu und öffnet den PR über den Fork.
Voraussetzungen:

- Der erste Eintrag ist in winget-pkgs aufgenommen.
- Der Fork `crazmoe/winget-pkgs` existiert.
- Repo-Secret `WINGET_TOKEN` im StarCLX-Repo: ein *classic* Personal Access Token mit dem Recht
  `public_repo` (Settings › Developer settings › Personal access tokens › Tokens (classic)).

Ohne Secret, oder solange `crazmoe.StarCLX` in winget-pkgs noch nicht aufgenommen ist, überspringt
die CI den Schritt. Ein Release aus dieser Zeit lässt sich nach der Aufnahme nachmelden, indem man
den Job `winget` seines Laufs neu startet.

## Alten Eintrag crazmoe.StarCMX entfernen

Erst wenn `crazmoe.StarCLX` aufgenommen ist, damit Benutzer nicht ohne Paket dastehen:

```
winget show crazmoe.StarCMX --versions
wingetcreate delete crazmoe.StarCMX --version <version> --reason "Renamed to crazmoe.StarCLX" --token <token>
```

Für jede aufgeführte Version einmal. Bereits installierte StarCMX laufen weiter; ein
`winget install crazmoe.StarCLX` ersetzt sie.
