---
id: guides/install
title: Installing buzz-router
doc-type: runbook
status: draft
component: buzz-router
owner: dpalfery
last-reviewed: 2026-10-08
---

# Installing buzz-router

How to put the `buzz-router` binary on a machine that hosts bots, check it, upgrade it
and remove it. Installing does not configure anything: for that, continue with
[Getting started](getting-started.md).

Each release is built for five targets:

| OS | Architecture | Target |
|---|---|---|
| macOS | Apple silicon | `aarch64-apple-darwin` |
| macOS | Intel | `x86_64-apple-darwin` |
| Linux (glibc) | x86_64 | `x86_64-unknown-linux-gnu` |
| Linux (glibc) | aarch64 | `aarch64-unknown-linux-gnu` |
| Windows | x86_64 | `x86_64-pc-windows-msvc` |

## Install

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.ps1 | iex
```

The script finds the latest **stable** release, downloads the archive for this machine,
checks it against the release's `SHA256SUMS.txt`, and installs it. It refuses to install
anything whose checksum does not match. It needs no sudo, no Rust and no .NET.

| | Default install location |
|---|---|
| macOS, Linux | `~/.local/bin/buzz-router` |
| Windows | `%LOCALAPPDATA%\buzz-router\bin\buzz-router.exe` (added to your user `PATH`) |

On macOS and Linux the script prints the `export PATH=...` line to add to your shell
profile when the install directory is not already on `PATH`. On Windows it edits your user
`PATH`; open a new terminal afterwards.

Check it worked:

```bash
buzz-router --version
```

## Choose a version

Releases are tagged `vX.Y.Z`. Release candidates are tagged `vX.Y.Z-rc.N` and published
as GitHub pre-releases, so a plain install never picks one.

| You want | macOS, Linux | Windows |
|---|---|---|
| A specific version | `... \| sh -s -- --version 0.1.0` | `$env:BUZZ_ROUTER_VERSION='0.1.0'; irm ... \| iex` |
| A release candidate | `... \| sh -s -- --version 0.1.0-rc.2` | `$env:BUZZ_ROUTER_VERSION='0.1.0-rc.2'; irm ... \| iex` |
| The newest release, candidates included | `... \| sh -s -- --prerelease` | `$env:BUZZ_ROUTER_PRERELEASE='1'; irm ... \| iex` |
| A different directory | `... \| sh -s -- --install-dir /usr/local/bin` | `$env:BUZZ_ROUTER_INSTALL_DIR='C:\tools'; irm ... \| iex` |

The same options exist as environment variables for the shell script:
`BUZZ_ROUTER_VERSION`, `BUZZ_ROUTER_PRERELEASE=1` and `BUZZ_ROUTER_INSTALL_DIR`.

### Pin the installer too

The commands above fetch the installer from `main`. To use the exact installer a release
was cut with, take it from that release:

```bash
curl -fsSL https://github.com/dpalfery/buzz-router/releases/download/v0.1.0/install.sh | sh -s -- --version 0.1.0
```

```powershell
$env:BUZZ_ROUTER_VERSION='0.1.0'; irm https://github.com/dpalfery/buzz-router/releases/download/v0.1.0/install.ps1 | iex
```

## Verify a download by hand

Every release lists a SHA-256 for each asset in `SHA256SUMS.txt`. To check a manually
downloaded archive, put it and `SHA256SUMS.txt` in one folder:

```bash
# Linux
sha256sum --check --ignore-missing SHA256SUMS.txt
# macOS
shasum -a 256 --check --ignore-missing SHA256SUMS.txt
```

```powershell
# Windows: compare this against the matching line in SHA256SUMS.txt
Get-FileHash -Algorithm SHA256 .\buzz-router-0.1.0-x86_64-pc-windows-msvc.zip
```

The binaries are not code-signed or notarized, so the checksum is the integrity check.
On Windows, SmartScreen may warn the first time you run an unsigned `.exe`.

## Upgrade

Run the installer again. It replaces the binary in place and prints
`upgraded: <old> -> <new>`.

A running router keeps using the old binary until it restarts. The installed service
records the binary's path, so after an upgrade restart it:

```bash
buzz-router service uninstall
buzz-router service install
buzz-router service status
```

Do this on every machine, one at a time. Check that `buzz-router roster check` still
prints the same hash everywhere afterwards.

## Uninstall

```bash
buzz-router service uninstall
rm ~/.local/bin/buzz-router        # Windows: delete %LOCALAPPDATA%\buzz-router\bin
```

This leaves the config directory, the data directory and the keychain entries alone, so a
reinstall picks up where you left off. Delete those yourself only if you mean to start over.

## Bots that call `buzz-router`

A bot that replies through the router runs `buzz-router post`, `pass` or `eta` from inside
its wake (`reply_mode = "api"`). The router starts that process, and a router started by
launchd, systemd or Task Scheduler often has a shorter `PATH` than your terminal, so the
installed binary may not be found.

Give the bot's adapter a `PATH` that includes the install directory, in `router.toml`:

```toml
[bots.adapter]
env = { PATH = "/Users/<you>/.local/bin:/usr/local/bin:/usr/bin:/bin" }
```

Setting `PATH` there replaces the inherited one, so list every directory the command
needs, including the one holding the agent program itself.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `could not find a release` | No stable release exists yet. Use `--prerelease`, or `--version` with a candidate. |
| `checksum mismatch` | The download was corrupted or altered. Nothing was installed. Run it again; if it repeats, do not use that release. |
| `cannot download ... (does that release exist?)` | The version has no archive for this platform, or the tag is wrong. |
| `unsupported architecture` | Only the five targets above are built. Musl Linux, 32-bit and Windows on ARM are not. |
| `buzz-router: command not found` | The install directory is not on `PATH`. Add it, or open a new terminal on Windows. |
| Windows SmartScreen warning | Expected for an unsigned binary. Verify the checksum, then choose **Run anyway**. |
