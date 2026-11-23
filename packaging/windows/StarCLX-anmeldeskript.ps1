# StarCLX per GPO-Anmeldeskript installieren oder aktualisieren
# Läuft als angemeldeter Benutzer (StarCLX installiert pro Benutzer).
# Die Version liest es aus version.txt, die StarCLX-herunterladen.ps1 auf der Freigabe pflegt.
# Protokoll: %TEMP%\StarCLX-gpo.log beim jeweiligen Benutzer
# Anpassen: Pfad zur internen Freigabe
$Freigabe = '\\SERVER\Software\StarCLX'

$Log = Join-Path $env:TEMP 'StarCLX-gpo.log'
function Log($t) { Add-Content $Log ("[{0:yyyy-MM-dd HH:mm:ss}] {1}" -f (Get-Date), $t) }
Log "Start als $env:USERDOMAIN\$env:USERNAME auf $env:COMPUTERNAME, PowerShell $($PSVersionTable.PSVersion)"

try {
    $VersionDatei = Join-Path $Freigabe 'version.txt'
    if (-not (Test-Path $VersionDatei)) { Log "Nicht erreichbar: $VersionDatei"; exit 0 }
    $Version = (Get-Content $VersionDatei -Raw).Trim()

    $Setup = Join-Path $Freigabe "StarCLX_${Version}_x64-setup.exe"
    $Key   = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\StarCLX'

    $Installiert = (Get-ItemProperty -Path $Key -ErrorAction SilentlyContinue).DisplayVersion
    Log "Freigabe: $Version, installiert: '$Installiert'"
    if ($Installiert -eq $Version) { Log "Schon aktuell"; exit 0 }
    if (-not (Test-Path $Setup)) { Log "Installer fehlt: $Setup"; exit 0 }

    # StarCLX beenden, falls es läuft (sonst kann das Update keine Dateien ersetzen),
    # ebenso StarCMX (so hiess StarCLX unter Windows bis 2.0; das Setup entfernt es)
    Get-Process StarCLX, starclx, StarCMX, starcmx -ErrorAction SilentlyContinue | Stop-Process -Force

    # Lokal kopieren, ohne Internet-Kennzeichnung, dann still installieren
    $Tmp = Join-Path $env:TEMP 'StarCLX-setup.exe'
    Copy-Item $Setup $Tmp -Force
    Unblock-File $Tmp
    $p = Start-Process -FilePath $Tmp -ArgumentList '/S' -Wait -PassThru
    Log "Installer beendet, Exitcode $($p.ExitCode)"
    Remove-Item $Tmp -Force -ErrorAction SilentlyContinue

    $Neu = (Get-ItemProperty -Path $Key -ErrorAction SilentlyContinue).DisplayVersion
    Log "Jetzt installiert: '$Neu'"
} catch {
    Log "Fehler: $($_.Exception.Message)"
}
