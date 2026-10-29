# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for a security problem. Report it privately with GitHub: open this repository's **Security** tab, then **Report a vulnerability**. Include what you found, how to reproduce it, and which version (`uppwind-companion --help` prints it) and which system (Windows build, or macOS version) you used.

You will get an answer as soon as possible. A fix is released first, then the report is published with credit to you, unless you prefer otherwise.

## Scope

In scope: the Uppwind Link companion (`companion/`, Windows and macOS), the Uppwind Link plugins (`plugin/`), the scripts for Ableton Live and FL Studio (`ableton/UppwindLink/`, `fl-studio/`, `shared/`) and the protocols between them and Uppwind Studio. Examples: a page other than the studio's that can drive the companion; a way around the pairing code or the plugins' secret; a way to make the companion show, click or capture anything other than a DAW's plugin windows and the DAW's own sound; anything that reads or writes files, or runs a program; a way to make the companion's copy with administrator rights (Windows: the plugins' folder only) write, remove or run anything else; a crash caused by a message.

Not in this repository, but welcome the same way: problems in the Uppwind Studio website. Out of scope: programs already running under your own account (they can read the pairing code like any of your files: see *Security model* in the README), and attacks that need administrator rights on your computer.

## Supported versions

Only the latest release gets security fixes.

## Before you share a log or a console output

The companion never writes the pairing code to its background log. Its console window shows the code once, so that you can copy it: remove any line starting with `UWC2-` (or `UWC-`, older companions) before you paste console output anywhere. Logs also contain the names of your plugins and tracks.
