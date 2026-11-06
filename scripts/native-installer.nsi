Unicode True
!include "MUI2.nsh"
!include "FileFunc.nsh"
Name "Monoptah Native"
OutFile "@OUTFILE@"
InstallDir "$LOCALAPPDATA\Programs\Monoptah Native"
InstallDirRegKey HKCU "Software\Monoptah Native" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "@WINDOWS_VERSION@"
VIAddVersionKey "ProductName" "Monoptah Native"
VIAddVersionKey "FileDescription" "Monoptah Native preview installer"
VIAddVersionKey "FileVersion" "@VERSION@"
!define MUI_ICON "@ICON@"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Var UpdatePID
Var HadPrevious
Function .onInit
  ${GetParameters} $R0
  ${GetOptions} $R0 "/UPDATEPID=" $UpdatePID
  StrCmp $UpdatePID "" ready
  System::Call 'kernel32::OpenProcess(i 0x00100000, i 0, i $UpdatePID) p .r1'
  StrCmp $1 0 ready
  System::Call 'kernel32::WaitForSingleObject(p r1, i 120000) i .r2'
  System::Call 'kernel32::CloseHandle(p r1)'
  StrCmp $2 0 ready
  Abort "Monoptah Native did not close. Please try updating again."
  ready:
FunctionEnd
Section "Monoptah Native"
  SetOutPath "$INSTDIR"
  ClearErrors
  File /oname=monocode-gpui.next.exe "@STAGE@\monocode-gpui.exe"
  IfErrors install_failed
  StrCpy $HadPrevious 0
  IfFileExists "$INSTDIR\monocode-gpui.exe" 0 replace_binary
  Delete "$INSTDIR\monocode-gpui.previous.exe"
  ClearErrors
  Rename "$INSTDIR\monocode-gpui.exe" "$INSTDIR\monocode-gpui.previous.exe"
  IfErrors install_failed
  StrCpy $HadPrevious 1
  replace_binary:
  ClearErrors
  Rename "$INSTDIR\monocode-gpui.next.exe" "$INSTDIR\monocode-gpui.exe"
  IfErrors rollback_binary
  File "@STAGE@\native-install.json"
  File /r "@STAGE@\licenses"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Monoptah Native" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MonoptahNative" "DisplayName" "Monoptah Native"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MonoptahNative" "DisplayVersion" "@VERSION@"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MonoptahNative" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  CreateShortcut "$SMPROGRAMS\Monoptah Native.lnk" "$INSTDIR\monocode-gpui.exe"
  CreateShortcut "$DESKTOP\Monoptah Native.lnk" "$INSTDIR\monocode-gpui.exe"
  Delete "$INSTDIR\monocode-gpui.previous.exe"
  Goto install_done
  rollback_binary:
  StrCmp $HadPrevious 1 0 install_failed
  Rename "$INSTDIR\monocode-gpui.previous.exe" "$INSTDIR\monocode-gpui.exe"
  install_failed:
  Delete "$INSTDIR\monocode-gpui.next.exe"
  MessageBox MB_ICONSTOP "Monoptah Native could not be updated. The previous executable was retained. Close other Monoptah Native processes and try again."
  Abort
  install_done:
SectionEnd
Function .onInstFailed
  StrCmp $UpdatePID "" failed_done
  IfFileExists "$INSTDIR\monocode-gpui.exe" 0 failed_done
  Exec '"$INSTDIR\monocode-gpui.exe"'
  failed_done:
FunctionEnd
Function .onInstSuccess
  StrCmp $UpdatePID "" done
  Exec '"$INSTDIR\monocode-gpui.exe"'
  done:
FunctionEnd
Section "Uninstall"
  Delete "$SMPROGRAMS\Monoptah Native.lnk"
  Delete "$DESKTOP\Monoptah Native.lnk"
  Delete "$INSTDIR\monocode-gpui.exe"
  Delete "$INSTDIR\native-install.json"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\licenses"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Monoptah Native"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MonoptahNative"
SectionEnd
