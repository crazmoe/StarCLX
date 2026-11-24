# winget

Eintrag im [Windows Package Manager Community Repository](https://github.com/microsoft/winget-pkgs),
Paket-ID `crazmoe.StarCLX`. Danach installiert man StarCLX unter Windows mit

```
winget install crazmoe.StarCLX
```

Bis Version 2.0 hiess die Windows-Version StarCMX (`crazmoe.StarCMX`). Dieser Eintrag wurde nie
in winget-pkgs aufgenommen (der PR dazu wird geschlossen); StarCLX wird direkt als
`crazmoe.StarCLX` eingereicht. Das StarCLX-Setup entfernt ein installiertes StarCMX selbst und
übernimmt Einstellungen und Anmeldung.

## Erste Einreichung (einmalig, von Hand)

`manifests/` hat denselben Aufbau wie in winget-pkgs; der Ordner
`manifests/c/crazmoe/StarCLX/2.1.0/` wird dort 1:1 eingereicht, am einfachsten unter Windows mit

```
winget install wingetcreate
wingetcreate submit --token <token> packaging\winget\manifests\c\crazmoe\StarCLX\2.1.0
```

Alternativ im Fork `crazmoe/winget-pkgs` einen Branch anlegen, den Ordner an dieselbe Stelle
hochladen und einen PR gegen `microsoft/winget-pkgs` öffnen. Die Prüfung von Microsoft
(Installation in einer Test-VM, Virenscan) dauert meist ein bis drei Tage. Beim ersten PR
verlangt der Bot die Microsoft-CLA: im PR mit `@microsoft-github-policy-service agree`
antworten, sonst bleibt er mit dem Label `Needs-CLA` liegen.

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
