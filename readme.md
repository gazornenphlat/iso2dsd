# ISO2DSD

A small Windows/Linux desktop app that converts SACD disc images (`.iso`) into
DSD audio files: DSDIFF (Philips), DSDIFF Edit Master, Sony DSF, or raw ISO.

It is a Rust (eframe/egui) rewrite of the Java program `iso2dsd_gui.jar`. The
app does not decode audio itself. It runs the external converter `sacd_extract`
once per file.

## Requirements

- Rust (stable) with cargo, to build.
- The converter `sacd_extract.exe` (Windows) or `sacd_extract` (Linux/macOS),
  downloaded separately from the SACD-Ripper releases. Put it in the same folder
  as the built program. The footer shows "Converter ready" when it is found.

## Build and run

```
cargo build --release
cargo test
```

Build output is kept outside this synced folder. Set this once, then open a new
terminal (see `app/.cargo/config.toml`):

```
setx CARGO_TARGET_DIR "%LOCALAPPDATA%\iso2dsd\target"
```

Run the built program from the folder that holds `sacd_extract`.

The program also takes `--screenshot <file.png>`: it draws the window, saves a
PNG, and exits.

## Using it

1. Choose **File input** and click **Add ISO files...** (or drag `.iso` files
   onto the window). Use **Server input** to send a converter server address
   instead.
2. Set the options on the right:
   - **Channel mode:** Dual (stereo, `-2`) or Multi (multi-channel, `-m`).
   - **Output format:** Philips DSDIFF (`-p`), DSDIFF Edit Master (`-e`, one
     track), Sony DSF (`-s`), or Raw ISO (`-I`).
   - **Convert DST to DSD** (`-c`), **Write a cue sheet** (`-C`), **Print disc
     information** (`-P`).
3. Click **Convert**. Click **Stop** to end the current file and skip the rest.
   Files already written are kept.

The log shows each file as it starts, a progress bar for the current file when
the converter reports progress, and the files it wrote with their sizes.

Output is written **next to each source ISO**. Server input writes to the
program's folder.

Settings (options, last folder, server address) are remembered between runs.

## Behaviour notes

- Flags are added in this order: `-c`, `-C`, `-P`, channel, format, `--input`.
- The converter runs once per selected file, one after another.
- If a run finishes without writing any file in the output folder, the log says
  so. Only files directly in that folder are checked, not subfolders.
- **Check for Updates** opens the project website. It does not download or
  replace the program.
- Drag and drop and the file list accept only `.iso` files.

## Known gaps

- Progress parsing assumes the converter prints `Processing n of m` or
  `Completed: n%`. This has not been confirmed against the real converter.
- The meaning of `-C`, `-P` and `-I` is unconfirmed. Run `sacd_extract -?` with
  the real converter to check.
- File names with accented letters should work (the original Java program had a
  bug there), but this has not been tested on this machine.

## Folder layout

| Path | What it is |
|---|---|
| `app/` | The Rust application (`src/main.rs` UI, `src/convert.rs` converter runner, `Cargo.toml`). |
| `app/MECHANICS.md`, `app/mechanics.json` | The original program's values and rules, with where each came from. |
| `investigation.cany/` | Notes from the investigation of the original jar. Managed by that tool (`project.toml`, evidence IDs); edit with care. |
| `checks/` | Checks of the converter's documentation and two project web pages. |
| `capture/` | Capture notes. |
| `app/target`, `app/target-new` | Old build output. Safe to delete once the program is not running. |

## Licensing

The conversion is based on SACD-Ripper (GNU GPL v2). The original Java interface
is copyright Simple Design, LLC. This app contains no artwork or copied text from
the original. If `sacd_extract` is bundled with a distributed copy of this app,
GPL terms apply to it.
