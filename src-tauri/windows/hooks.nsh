; PoxiMP as the music player for audio files (bundle.windows.nsis.installerHooks).
; One ProgID per format with its own icon ($INSTDIR\icons\files\*.ico, from bundle.resources),
; "Open with" entries and Capabilities + RegisteredApplications, so PoxiMP is listed in
; Settings > Default apps. The defaults stay the user's choice (Windows 10/11 protect them):
; Ajustes > General in the app opens that page. SHCTX = HKCU or HKLM, as installMode says.
; Saved as UTF-8 with BOM: NSIS reads the Spanish names right only with it.

!define POXIMP_CAPS "Software\PoxiMP\Capabilities"

!macro POXIMP_TYPE PROGID ICON NAME
  WriteRegStr SHCTX "Software\Classes\${PROGID}" "" "${NAME}"
  WriteRegStr SHCTX "Software\Classes\${PROGID}" "FriendlyTypeName" "${NAME}"
  WriteRegStr SHCTX "Software\Classes\${PROGID}\DefaultIcon" "" "$INSTDIR\icons\files\${ICON}.ico,0"
  WriteRegStr SHCTX "Software\Classes\${PROGID}\shell" "" "open"
  WriteRegStr SHCTX "Software\Classes\${PROGID}\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
!macroend

!macro POXIMP_EXT EXT PROGID
  WriteRegStr SHCTX "Software\Classes\.${EXT}\OpenWithProgids" "${PROGID}" ""
  WriteRegStr SHCTX "${POXIMP_CAPS}\FileAssociations" ".${EXT}" "${PROGID}"
!macroend

; Only our value: .ext and OpenWithProgids are shared with other apps (and DeleteRegKey /ifempty
; ignores values, so it could drop theirs).
!macro POXIMP_UNEXT EXT PROGID
  DeleteRegValue SHCTX "Software\Classes\.${EXT}\OpenWithProgids" "${PROGID}"
!macroend

; Explorer redraws the icons (SHCNE_ASSOCCHANGED, SHCNF_IDLIST | SHCNF_FLUSH).
!macro POXIMP_REFRESH
  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, p 0, p 0)"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro POXIMP_TYPE "PoxiMP.mp3" "mp3" "Audio MP3"
  !insertmacro POXIMP_TYPE "PoxiMP.flac" "flac" "Audio FLAC"
  !insertmacro POXIMP_TYPE "PoxiMP.wav" "wav" "Audio WAV"
  !insertmacro POXIMP_TYPE "PoxiMP.aiff" "aiff" "Audio AIFF"
  !insertmacro POXIMP_TYPE "PoxiMP.m4a" "m4a" "Audio M4A"
  !insertmacro POXIMP_TYPE "PoxiMP.aac" "aac" "Audio AAC"
  !insertmacro POXIMP_TYPE "PoxiMP.ogg" "ogg" "Audio OGG"
  !insertmacro POXIMP_TYPE "PoxiMP.opus" "opus" "Audio Opus"
  !insertmacro POXIMP_TYPE "PoxiMP.wv" "wv" "Audio WavPack"
  !insertmacro POXIMP_TYPE "PoxiMP.ape" "ape" "Audio APE"
  !insertmacro POXIMP_TYPE "PoxiMP.m3u" "m3u" "Lista de reproducción M3U"

  !insertmacro POXIMP_EXT "mp3" "PoxiMP.mp3"
  !insertmacro POXIMP_EXT "flac" "PoxiMP.flac"
  !insertmacro POXIMP_EXT "wav" "PoxiMP.wav"
  !insertmacro POXIMP_EXT "aif" "PoxiMP.aiff"
  !insertmacro POXIMP_EXT "aiff" "PoxiMP.aiff"
  !insertmacro POXIMP_EXT "m4a" "PoxiMP.m4a"
  !insertmacro POXIMP_EXT "aac" "PoxiMP.aac"
  !insertmacro POXIMP_EXT "ogg" "PoxiMP.ogg"
  !insertmacro POXIMP_EXT "oga" "PoxiMP.ogg"
  !insertmacro POXIMP_EXT "opus" "PoxiMP.opus"
  !insertmacro POXIMP_EXT "wv" "PoxiMP.wv"
  !insertmacro POXIMP_EXT "ape" "PoxiMP.ape"
  !insertmacro POXIMP_EXT "m3u" "PoxiMP.m3u"
  !insertmacro POXIMP_EXT "m3u8" "PoxiMP.m3u"

  WriteRegStr SHCTX "${POXIMP_CAPS}" "ApplicationName" "PoxiMP"
  WriteRegStr SHCTX "${POXIMP_CAPS}" "ApplicationDescription" "Reproductor de música local"
  WriteRegStr SHCTX "${POXIMP_CAPS}" "ApplicationIcon" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  ; The value name is what the app's "default apps" button opens (registeredAppUser=PoxiMP).
  WriteRegStr SHCTX "Software\RegisteredApplications" "PoxiMP" "${POXIMP_CAPS}"
  ; Name in "Open with" (else the exe's file description).
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe" "FriendlyAppName" "PoxiMP"
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  !insertmacro POXIMP_REFRESH
!macroend

; Updates run the old uninstaller first: keep everything then, or Windows could reset the user's PoxiMP
; defaults while the ProgIDs are briefly gone.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.mp3"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.flac"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.wav"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.aiff"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.m4a"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.aac"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.ogg"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.opus"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.wv"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.ape"
    DeleteRegKey SHCTX "Software\Classes\PoxiMP.m3u"

    !insertmacro POXIMP_UNEXT "mp3" "PoxiMP.mp3"
    !insertmacro POXIMP_UNEXT "flac" "PoxiMP.flac"
    !insertmacro POXIMP_UNEXT "wav" "PoxiMP.wav"
    !insertmacro POXIMP_UNEXT "aif" "PoxiMP.aiff"
    !insertmacro POXIMP_UNEXT "aiff" "PoxiMP.aiff"
    !insertmacro POXIMP_UNEXT "m4a" "PoxiMP.m4a"
    !insertmacro POXIMP_UNEXT "aac" "PoxiMP.aac"
    !insertmacro POXIMP_UNEXT "ogg" "PoxiMP.ogg"
    !insertmacro POXIMP_UNEXT "oga" "PoxiMP.ogg"
    !insertmacro POXIMP_UNEXT "opus" "PoxiMP.opus"
    !insertmacro POXIMP_UNEXT "wv" "PoxiMP.wv"
    !insertmacro POXIMP_UNEXT "ape" "PoxiMP.ape"
    !insertmacro POXIMP_UNEXT "m3u" "PoxiMP.m3u"
    !insertmacro POXIMP_UNEXT "m3u8" "PoxiMP.m3u"

    DeleteRegValue SHCTX "Software\RegisteredApplications" "PoxiMP"
    DeleteRegKey SHCTX "Software\PoxiMP"
    DeleteRegKey SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe"
    !insertmacro POXIMP_REFRESH
  ${EndIf}
!macroend
