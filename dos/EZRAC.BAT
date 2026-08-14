@echo off

if "%1"=="" goto usage

if "%2"=="" goto usage

if /I "%2"=="EZRAC.EZI" goto reserved

if /I "%2"=="EZRAC.EZO" goto reserved

if /I "%2"=="EZRAC.ASM" goto reserved

EZFE.EXE "%1" EZRAC.EZI

if errorlevel 1 goto failed

EZOPT.EXE EZRAC.EZI EZRAC.EZO

if errorlevel 1 goto failed

EZCG.EXE EZRAC.EZO EZRAC.ASM

if errorlevel 1 goto failed

EZAS.EXE EZRAC.ASM "%2"

if errorlevel 1 goto failed

del EZRAC.EZI >NUL

del EZRAC.EZO >NUL

del EZRAC.ASM >NUL

goto done

:failed

echo EZRAC failed.

goto done

:reserved

echo output name is reserved for an EZRAC temporary file.

goto done

:usage

echo usage: EZRAC source.ezra output.com

:done
