@echo off
chcp 65001 >nul
title PoxiMP - desarrollo
cd /d "%~dp0"
set ELECTRON_RUN_AS_NODE=

where node >nul 2>&1 || (echo No se encuentra Node.js en el PATH. & pause & exit /b 1)
where cargo >nul 2>&1 || (echo No se encuentra Rust/cargo en el PATH. & pause & exit /b 1)
where cmake >nul 2>&1 || (echo No se encuentra CMake en el PATH ^(hace falta para compilar Opus^). & pause & exit /b 1)

if not exist node_modules (
  echo Instalando dependencias...
  call npm install || (echo Fallo npm install. & pause & exit /b 1)
)

echo Arrancando PoxiMP en modo desarrollo...
echo La primera compilacion de Rust tarda unos minutos; las siguientes, segundos.
echo Los cambios en la interfaz se ven al instante sin reiniciar.
echo.
call npm run dev
pause
