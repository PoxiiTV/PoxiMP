@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul
title PoxiMP - generar distribuibles
cd /d "%~dp0"

echo.
echo  ==========================================================
echo    PoxiMP  -  instalador + portable
echo  ==========================================================
echo.

REM ---------------------------------------------------------------- requisitos
where node >nul 2>&1
if errorlevel 1 (
  echo  [X] No se encuentra Node.js en el PATH.
  goto :fin_error
)
where cargo >nul 2>&1
if errorlevel 1 (
  echo  [X] No se encuentra Rust/cargo en el PATH.
  goto :fin_error
)

REM Si existe .env (p. ej. la clave que firma las actualizaciones), se carga
if exist ".env" (
  for /f "usebackq eol=# tokens=1,* delims==" %%a in (".env") do set "%%a=%%b"
)

set "CLI=node_modules\.bin\tauri.cmd"
if not exist "%CLI%" (
  echo  [1/3] Instalando dependencias de Node...
  call npm install
  if errorlevel 1 goto :fin_error
  echo.
)

for /f "delims=" %%v in ('node -p "require('./src-tauri/tauri.conf.json').version"') do set "VER=%%v"

REM ------------------------------------------------------------------ compilar
echo  [2/3] Compilando PoxiMP %VER% en modo release...
echo        La primera vez tarda varios minutos (LTO para que el exe quede pequeno).
echo.
call "%CLI%" build
if errorlevel 1 (
  echo.
  echo  [X] La compilacion ha fallado. Revisa los mensajes de arriba.
  goto :fin_error
)

REM ------------------------------------------------------------------ recoger
set "REL=target\release"
set "NSIS=%REL%\bundle\nsis"
set "OUT=deploy-hosting"
set "EXE=%REL%\poximp.exe"
set "SETUP=%NSIS%\PoxiMP_%VER%_x64-setup.exe"

if not exist "%EXE%" (
  echo  [X] No se ha generado el ejecutable en %REL%
  goto :fin_error
)
if not exist "%SETUP%" (
  echo  [X] No se ha generado el instalador en %NSIS%
  goto :fin_error
)

echo.
echo  [3/3] Preparando %OUT%...
if exist "%OUT%" rmdir /s /q "%OUT%"
mkdir "%OUT%"

copy /y "%SETUP%" "%OUT%\PoxiMP-Setup-%VER%.exe" >nul || goto :fin_error
copy /y "%EXE%" "%OUT%\PoxiMP-%VER%-portable.exe" >nul || goto :fin_error
if exist "%SETUP%.sig" copy /y "%SETUP%.sig" "%OUT%\PoxiMP-Setup-%VER%.exe.sig" >nul
REM latest.json: lo que lee la actualizacion automatica (subelo a la release junto al instalador)
node scripts\latest-json.mjs || goto :fin_error
REM El .env NO se copia: aqui solo guarda la clave privada que firma las actualizaciones y
REM deploy-hosting se sube tal cual a la release publica.

REM --------------------------------------------------------------------- pesos
echo.
echo  ==========================================================
echo    Listo:  %OUT%
echo  ==========================================================
powershell -NoProfile -Command "Get-ChildItem '%OUT%' -File | ForEach-Object { '   {0,-36} {1,8:N2} MB' -f $_.Name, ($_.Length/1MB) }"
echo.
echo    Referencia: el exe ronda los 15 MB. Si pasa de ~30 MB, algo va mal en el build.
echo.
pause
endlocal
goto :eof

:fin_error
echo.
pause
endlocal
exit /b 1
