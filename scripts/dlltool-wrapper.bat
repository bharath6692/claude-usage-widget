@echo off
rem Forwards to rustup's bundled dlltool, naming the assembler explicitly.
rem CLAUDE_WIDGET_AS is set by scripts/cargo.ps1 (see docs/DESIGN.md "Toolchain notes").
"%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained\dlltool.exe" %* -S "%CLAUDE_WIDGET_AS%"
