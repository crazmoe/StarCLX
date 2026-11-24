; Zusätze zum NSIS-Setup von Tauri (bundle.windows.nsis.installerHooks)
;
; StarCLX hiess unter Windows bis Version 2.0 „StarCMX“. Das Setup entfernt
; ein installiertes StarCMX still, bevor es StarCLX einrichtet. Einstellungen,
; Konten und die gespeicherte Anmeldung bleiben: StarCLX nutzt unter Windows
; dieselben Datenordner (ch.crazmoe.starcmx) und denselben Eintrag in der
; Anmeldeinformationsverwaltung. Das alte Deinstallationsprogramm löscht
; ohne Rückfrage keine Daten.
;
; Es läuft vor dem Kopieren der Dateien, damit es die Rufnummern-Links
; (tel:, callto:, sip:) entfernt, bevor StarCLX sie neu einträgt.

!include "LogicLib.nsh"
!include "FileFunc.nsh"

!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarCMX" "UninstallString"
  ${If} $R0 != ""
    DetailPrint "StarCMX wird entfernt (heisst jetzt StarCLX)"
    ; Laufendes StarCMX beenden, sonst bleiben Dateien gesperrt
    nsExec::Exec 'taskkill /F /IM StarCMX.exe'
    Pop $R1
    ; Anführungszeichen um den Pfad entfernen
    StrCpy $R1 $R0 1
    ${If} $R1 == '"'
      StrCpy $R0 $R0 "" 1
      StrCpy $R0 $R0 -1
    ${EndIf}
    ${GetParent} $R0 $R2
    ${If} ${FileExists} $R0
      ; _?= lässt das Deinstallationsprogramm am Ort laufen, so wartet ExecWait
      ExecWait '"$R0" /S _?=$R2' $R1
      DetailPrint "StarCMX entfernt (Rückgabe $R1)"
      Delete $R0
      RMDir $R2
    ${EndIf}
    ; Falls das alte Deinstallationsprogramm fehlte: Eintrag trotzdem weg
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarCMX"
  ${EndIf}
!macroend
