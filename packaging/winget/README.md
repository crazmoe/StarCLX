# winget

Eintrag im [Windows Package Manager Community Repository](https://github.com/microsoft/winget-pkgs),
Paket-ID `crazmoe.StarCLX`. Danach installiert man StarCLX unter Windows mit

```
winget install crazmoe.StarCLX
```

Bis Version 2.0 hiess die Windows-Version StarCMX (`crazmoe.StarCMX`). Dieser Eintrag wurde nie
in winget-pkgs aufgenommen; es gibt also nichts umzustellen.

## Erste Einreichung (einmalig, von Hand)

Unter Windows, nach dem ersten Release mit Windows-Setup:

```
winget install wingetcreate
wingetcreate new https://github.com/crazmoe/StarCLX/releases/download/v<version>/StarCLX_<version>_x64-setup.exe
```

`wingetcreate` liest Name, Version und Prüfsumme aus dem Setup und fragt den Rest ab
(Herausgeber `crazmoe`, Lizenz MIT, Scope `user`, Protokolle `tel`, `callto`, `sip`). Mit
`--submit --token <token>` öffnet es den PR bei `microsoft/winget-pkgs` gleich selbst. Die
Prüfung von Microsoft (Installation in einer Test-VM, Virenscan) dauert meist ein bis drei Tage.

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
