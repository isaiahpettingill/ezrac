@echo off

echo Installing EZRAC for FreeDOS in C:\EZRAC

if not exist C:\EZRAC md C:\EZRAC

copy EZFE.EXE C:\EZRAC\EZFE.EXE >NUL

copy EZOPT.EXE C:\EZRAC\EZOPT.EXE >NUL

copy EZCG.EXE C:\EZRAC\EZCG.EXE >NUL

copy EZAS.EXE C:\EZRAC\EZAS.EXE >NUL

copy EZRAC.BAT C:\EZRAC\EZRAC.BAT >NUL

copy HELLO.EZR C:\EZRAC\HELLO.EZR >NUL

C:

cd \EZRAC

echo.

echo Compiling HELLO.EZR...

call EZRAC.BAT HELLO.EZR HELLO.COM

if errorlevel 1 goto failed

echo.

echo Running HELLO.COM...

HELLO.COM

goto done

:failed

echo HELLO.EZR did not compile.

:done

