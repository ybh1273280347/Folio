@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\package.ps1" -Launch
if errorlevel 1 pause
