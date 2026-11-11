# ISO2DSD: what the original does (values and rules)

Source: `iso2dsd_gui.jar` (main class `com.wilson.kevin.sacd_extract.Main`), read with the built-in
Java decompiler. Java class and method names are given where a value comes from. Pseudo-Java is
the decompiler's best guess; the bytecode is the evidence.

## Where each value comes from (decompiled Java; native_function_pseudocode does not apply to a Java archive)

Source text: Vineflower decompile of the jar (evidence ev_258e74cd364a6af763ddd9d98d6b0a7dbcde64c3de9f72f6c7c898ed9eecd4d5). Sizes are bytecode lengths from the class reader.

- `SacdExtractGui.init()` (1771 bytes): window size 850x650, the window controls, Help, Check for Updates, Execute, Quit, default port 2002.
- `SacdExtractGui.setPreviousPreferences()` (249 bytes): remembered defaults (Dual, Philips DSDIFF, all options off, File input, 0.0.0.0:2002).
- `SacdExtractGui.runHelp()` (203 bytes): runs the converter with `-?`, shows the output in a Help window.
- `SacdExtractGui$ConversionRunable.run()` (1033 bytes): flag order (-c, -C, -P, channel, output, --input), the "Please select a file(s) to convert." message, the per-file loop, and the server path (`--input ip:port`).
- `SacdExtractGui.versionUpdate()` (478 bytes): the update check. Not copied (see Design choices).
- `ProcessRunner.runCommand(List, JTextArea)` (269 bytes): output handling (progress lines, "We are done", the "[2K" removal). Not re-read in the decompiler this session; the rules come from the bytecode read.
- `OutputMode.toString()`, `ChannelMode.toString()` and `fromString`: the display names and the saved names.
- `IPField$1.verify` (62 bytes): octet clamp 0 to 255. `IPField.getIPAddressPort()` (51 bytes): port left out when empty.

Native pseudocode tools only read machine code, so they could not be used on this program. The decompiled source above is the evidence for each value.

## Window and screens

- Window: 850 x 650 (`SacdExtractGui.init`, setPreferredSize). Title "Sonore - ISO2DSD" in the original; this build uses "ISO2DSD".
- Start-up waits 5000 ms before showing the window (`Main.main`, Thread.sleep). **Not copied.**
- Top: input choice, "File Input" (default) or "Server Input" (`SacdExtractGui.init`, `setPreviousPreferences`).
- File input: a file chooser that only shows `.iso` files and allows several selections (`init`, FileNameExtensionFilter "iso").
- Server input: "IP Address | Port:" with four octet boxes and a port box (`init`, `IPField`).
- Right: "Options" with Channel Mode, Output Mode, and three check boxes: Cue Sheet, Convert DST to DSD, Print (`init`).
- Buttons: Execute, Help, Check for Updates, Quit (`init`).
- Log area in the middle (JTextArea, `init`).
- Footer: copyright line and the program version read from a `version.txt` beside the jar (`VersionReader.readVersion`). This build shows its own version instead.

## Defaults and remembered settings

| Setting | Default | Source |
|---|---|---|
| Input | File input | `setPreviousPreferences`: INPUT_TYPE "FILE" |
| Server address | 0.0.0.0:2002 | `setPreviousPreferences`: SERVER "0.0.0.0:2002"; `init`: setPort("2002") |
| Channel mode | Dual | `setPreviousPreferences`: CHANNEL_MODE = DUAL |
| Output mode | Philips DSDIFF | `setPreviousPreferences`: OUTPUT_MODE = PHILLIPS_DSDIFF |
| Convert DST to DSD | off | `setPreviousPreferences`: getBoolean("DSD", false) |
| Cue Sheet | off | `setPreviousPreferences`: getBoolean("CUE_SHEET", false) |
| Print | off | `setPreviousPreferences`: getBoolean("PRINT", false) |
| Folder | last folder used | `setPreviousPreferences`: DIR_KEY |

Remembered settings are kept in the user's preferences (`Preferences.userNodeForPackage`).

## Options and the converter command

Choices (in this order): Channel mode `Dual`, `Multi` (`ChannelMode.values`); Output mode `Philips DSDIFF`, `DSDIFF Edit Master`, `Sony DSF`, `Raw ISO` (`OutputMode.values`).

Each run calls the external converter (`sacd_extract.exe` on Windows, `./sacd_extract` elsewhere; `SacdExtractGui$ConversionRunable.run`, `SacdExtractGui.runHelp`). Flags are added in this order:

1. `-c` when Convert DST to DSD is ticked
2. `-C` when Cue Sheet is ticked
3. `-P` when Print is ticked
4. channel flag: Dual `-2`, Multi `-m`
5. output flag: Philips DSDIFF `-p`, DSDIFF Edit Master `-e`, Sony DSF `-s`, Raw ISO `-I`
6. `--input` followed by the file path (file input) or the server address `ip:port` (server input)

File input runs the converter once per selected file, one after another, in the program's own folder. Server input runs it once.

Help runs the converter with `-?` and shows its text in a "Help" window (`runHelp`).

Execute is disabled while a run is going and enabled again afterwards (`ConversionRunable.run`, setEnabled).

## Messages and output handling

- No file selected: message "Please select a file(s) to convert.", title "No file selected", warning icon (`ConversionRunable.run`).
- Converter output (both normal and error output are shown, `ProcessRunner.runCommand`):
  - ANSI "[2K" codes are removed.
  - A line that contains "Completed:" or "Processing" replaces the line before it (progress). **Changed:** the original replaces whatever line came last, even a normal line. This build only replaces a line that was also progress.
  - A line containing "We are done" gets an empty line before it.
  - The log scrolls to the newest line.
- Update check (`versionUpdate`): asks the author's web server for a newer zip (file name pattern `iso2dsd_<os>_v<n>.zip`), tries version numbers up to 100, downloads and replaces the program, then restarts it. Message when already current: "ISO2DSD is already up to date!", title "No Action Needed". **Changed:** this build opens the project website in the browser instead (https://www.sonore.us/iso2dsd.html, the page the person pointed to). Decided by the builder, since the person left this question to it.

## Bugs noticed in the original (not copied)

- The Print setting is always saved as on (`ConversionRunable.run`, `putBoolean("PRINT", 1)`). This build saves what you chose.
- Quit only closes the window (`SacdExtractGui$1`, dispose).

## Octet and port rules

- Octet box: empty becomes 0; below 0 becomes 0; above 255 becomes 255 (`IPField$1.verify`). Each octet box holds up to 3 digits (`IntegerRangeDocument(0, 999)`).
- Port box: up to 5 digits (`IntegerRangeDocument(0, 99999)`). An empty port means the IP address alone is sent (`IPField.getIPAddressPort`).

## What the project website says (evidence ev_9c6027d0ce530685eab128439314fe7bd252f18c588dd829daba6035e297f329)

- Features: DSD input in `.iso` files; "dual and multi-channel output"; "DFF and DSF output mode"; metadata copied from the ISO into the result; one file or many files in a folder.
- Setup: put the program and the converter (`sacd_extract`, downloaded separately from its GitHub releases) in the same folder.
- Mac: the converter may need execute permission (`chmod a+x sacd_extract`). Linux: the program needs execute permission too.
- Original notes a quirk: file names with accented letters can stop a conversion in the Java program. The Rust build passes file names as separate arguments, so this shouldn't apply (not tested on this machine).
- Licensing: the conversion is based on SACD-Ripper (GNU GPL v2); the Java interface is copyright Simple Design, LLC. Development is closed; provided as-is.

## Estimated (not confirmed in the code)

- Confirmed by the upstream converter's README usage examples (`checks/sacd-ripper-page`): Dual = stereo tracks (`-2`), Multi = multi-channel tracks (`-m`), `-c` = convert all DST to DSD, Philips DSDIFF / Sony DSF / Edit Master = the output files named in those examples (`-p`, `-s`, `-e`; Edit Master writes a single track).
- Estimated: "Print" (`-P`) asks the converter to print disc information, and "Cue Sheet" (`-C`) writes a cue sheet. The examples don't cover these two flags, nor Raw ISO (`-I`); the converter's own help text would settle them (unknown recorded, still open).

## Design choices in this build

- Same flags, same order, same defaults, same messages (except the changes noted above).
- Window opens at 850 x 650, the same as the original; it can be resized down to 720 x 540.
- Start-up wait of the original (5 s) and its update check are not copied (see the notes above).
- Converter is looked for in the program's own folder, as the original does.
- Drag and drop of `.iso` files added to the file list.
- "Check for Updates" opens the project website (`UPDATE_PAGE` in `src/main.rs`) and does not download or replace the program.
- No pictures, logos or the original's splash artwork are used; the style is new.
