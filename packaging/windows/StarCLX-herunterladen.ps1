# StarCLX: neueste Version von GitHub auf die interne Freigabe holen
# Läuft auf dem Server als geplante Aufgabe (z. B. täglich), mit Schreibrecht auf die Freigabe.
# Anpassen: Pfad zur Freigabe
$Freigabe = '\\SERVER\Software\StarCLX'

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$Log = Join-Path $Freigabe 'herunterladen.log'
function Log($t) { Add-Content $Log ("[{0:yyyy-MM-dd HH:mm}] {1}" -f (Get-Date), $t) }

try {
    $Release = Invoke-RestMethod 'https://api.github.com/repos/crazmoe/StarCLX/releases/latest' `
        -Headers @{ 'User-Agent' = 'StarCLX-Verteilung' }
    $Version = $Release.tag_name.TrimStart('v')
    $Asset   = $Release.assets | Where-Object { $_.name -like '*_x64-setup.exe' } | Select-Object -First 1
    if (-not $Asset) { Log "Kein Installer in Release $Version gefunden"; exit 1 }

    $VersionDatei = Join-Path $Freigabe 'version.txt'
    $Ziel = Join-Path $Freigabe $Asset.name
    if ((Test-Path $Ziel) -and (Test-Path $VersionDatei) -and
        ((Get-Content $VersionDatei -Raw).Trim() -eq $Version)) { exit 0 }   # schon aktuell

    $Tmp = "$Ziel.download"
    Invoke-WebRequest $Asset.browser_download_url -OutFile $Tmp -UseBasicParsing

    # Prüfsumme kontrollieren (GitHub liefert sie mit)
    if ($Asset.digest -match '^sha256:(.+)$') {
        $Ist = (Get-FileHash $Tmp -Algorithm SHA256).Hash
        if ($Ist -ne $Matches[1].ToUpper()) {
            Remove-Item $Tmp -Force
            Log "Prüfsumme falsch für $($Asset.name), nicht übernommen"; exit 1
        }
    }

    Move-Item $Tmp $Ziel -Force
    Unblock-File $Ziel
    Set-Content $VersionDatei $Version -Encoding ASCII     # erst jetzt sehen die Rechner die neue Version

    # alte Installer aufräumen
    Get-ChildItem $Freigabe -Filter 'Star*_x64-setup.exe' |
        Where-Object { $_.Name -ne $Asset.name -and $_.Name -match '^Star(CLX|CMX)_' } | Remove-Item -Force
    Log "StarCLX $Version heruntergeladen"
} catch {
    Log "Fehler: $($_.Exception.Message)"; exit 1
}
