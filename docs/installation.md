# Installing Libra

## Script installer

The recommended macOS/Linux installation is:

```bash
curl -fsSL https://download.libra.tools/install.sh | sh
```

The installer places `libra` in `$LIBRA_HOME/bin` (`~/.libra/bin` by default),
writes shell environment files, and creates the optional relative symlink:

```text
~/.libra/bin/lba -> libra
```

Both names execute the same binary. The relative target remains valid when the
whole Libra home directory is moved.

## Alias safety and idempotency

- A fresh install creates `lba` by default.
- Re-running the installer for the already-installed version repairs a missing
  alias without downloading or replacing `libra`.
- A valid `lba -> libra` or `lba -> $LIBRA_INSTALL_DIR/libra` symlink is
  accepted and refreshed to the relative form.
- A regular file, directory, or symlink to another target named `lba` is
  user-owned and is never overwritten. The installer prints a warning and
  continues.
- If the platform or filesystem cannot create symlinks, installing `libra`
  still succeeds. Use the full `libra` command after the warning.

The installer does not create a copy, hard link, shell function, or alias in a
profile. `lba` is only the optional filesystem symlink beside `libra`.

## Windows

Run the PowerShell installer from PowerShell:

```powershell
irm https://download.libra.tools/install.ps1 | iex
```

It installs `libra.exe` in `%USERPROFILE%\.libra\bin`, adds that directory to
the user PATH, and creates the optional `lba.cmd` shim. The shim resolves
`libra.exe` beside itself, so moving the `.libra` directory keeps both commands
working. Re-running the same version repairs a missing or Libra-managed shim
without replacing the binary.

Use `-NoAlias`, `LIBRA_NO_ALIAS=1`, or `-NoModifyPath` for opt-out behavior.
`-Uninstall` removes only the Libra-managed binary, shim, marker, and PATH
entry; existing user-owned `lba.*` files are preserved. The installer accepts
`-Version <VERSION>`, `-Dir <PATH>`, and `-BaseUrl <URL>` for scripted installs.

## Opting out

Use the flag for one invocation:

```bash
curl -fsSL https://download.libra.tools/install.sh | sh -s -- --no-alias
```

Or set the environment variable for automated installations:

```bash
curl -fsSL https://download.libra.tools/install.sh | LIBRA_NO_ALIAS=1 sh
```

The opt-out does not remove an existing alias; it only prevents the current
installer run from creating or refreshing one. Remove a known Libra-owned
symlink explicitly if desired:

```bash
test "$(readlink "$HOME/.libra/bin/lba" 2>/dev/null)" = libra &&
    rm "$HOME/.libra/bin/lba"
```

## Related options

| Option / variable | Effect |
|-------------------|--------|
| `-v`, `--version <VERSION>` | Install a specific release |
| `-d`, `--dir <PATH>` | Override the binary directory |
| `LIBRA_INSTALL_DIR` | Environment equivalent for the binary directory |
| `--no-modify-path` | Write env files but do not edit shell rc files |
| `--no-alias` | Do not create or refresh `lba` for this run |
| `LIBRA_NO_ALIAS=1` | Environment opt-out for `lba` |

Run `sh install.sh --help` from a checkout for the complete option and
environment-variable list.
