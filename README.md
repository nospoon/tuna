# Tuna

A Windows guitar/bass tuner built for the bottom end. Bass B0 (30.87 Hz) and
8-string F standard (F1, 43.65 Hz) lock in fast and then stay put: no octave
jumps, no needle hunting, no "which string am I on?" guessing.

Portable single `tuna.exe` — no installer, no runtime. Rust, with eframe/egui
for the UI and cpal (WASAPI) for capture. Pitch detection is correlation-based;
there is no FFT in the signal path.

## What it does

- Tracks fundamentals down to **27.5 Hz (A0)**, so B0, F1 and F♯1 are covered
  with margin.
- Anti-floating tracker: gate → clarity freeze → median → lock confirm →
  ±120 ¢ outlier clamp → EMA smoothing. The needle stops instead of sliding
  toward noise, and pinning a string chip restricts the search to that string.
- 15 tunings: chromatic, guitar standard/drop D/DADGAD/open G/half step/
  drop C/7-string low B/8-string F♯ and F, bass 4/4-drop D/5/6-string, and a
  custom editor. Sharp/flat spelling is display-only and never changes a number.
- A4 reference 435–455 Hz, in-tune dead band 1/3/5 ¢, needle smoothing
  fast/normal/slow, per-channel input selection.
- Diagnostics in the window: level, gate and noise floor, a 12-bar harmonic
  strip (a weak fundamental is called out instead of being tuned to), and hint
  lines (`raise interface gain`, `wrong tuning selected?`).
- Offline mode: analyse a WAV and print CSV, no GUI.

## Build

Requirements: Windows, a recent Rust (1.87+, for `u32::is_multiple_of`;
developed on 1.99.1) and an MSVC C++ toolchain with the Windows SDK, which
`cpal` needs to link.

```powershell
cargo build --release            # target\release\tuna.exe
cargo run --release              # run it
pwsh ./build.ps1                 # dist\tuna.exe + dist\tuna-win-x64.zip
```

The dev profile is `opt-level = 1` so live capture keeps up in a debug build.

## Command line

```
tuna                       start the GUI on the default input
--list-devices             print capture devices and exit
--list-presets             print tuning ids and exit
--device <substr>          device id or case-insensitive name fragment
--channel <n>              0-based input channel
--preset <id>              see --list-presets
--spelling sharp|flat      accidental spelling (display only)
--a4 <hz>                  reference pitch, 435..455
--wav <path>               analyse a file instead of the live input
--csv                      print analysis rows to stdout (headless)
--seconds <n>              live capture duration for --csv
--help                     this text
```

Both `--flag value` and `--flag=value` work. Headless example:

```powershell
cargo run --release -- --wav fixtures/f1.wav --preset guitar_8_f --csv
```

```
t_ms,state,f_hz,cents,clarity,target,rms_db,gate_db
576,Locked,43.691,1.49,0.974,29,-21.9,-49.0
1429,Locked,43.652,-0.04,0.971,29,-33.4,-45.0
```

`fixtures/` is generated, never committed:

```powershell
cargo run --release --bin wavgen -- --out fixtures/b0.wav --midi 23 --seconds 6 --onset 300 --harmonics 8 --noise -60 --seed 7
```

## Configuration

Saved to `%APPDATA%\tuna\config.toml` on change (debounced, atomic
write-and-rename). A corrupt file is renamed `config.toml.bak`, defaults are
used and the reason is shown in the window instead of failing startup. Unknown
keys are ignored, so an older file keeps loading.

|key|default|
|---|---|
|`device_id`|`""` — system default input|
|`input_channel`|`0`|
|`preset`|`"guitar_standard"`|
|`custom_tuning`|`[40, 45, 50, 55, 59, 64]` (MIDI, low → high)|
|`pinned_string`|none — follow the measured note|
|`a4_hz`|`440.0`|
|`spelling`|`"sharp"`|
|`tolerance_cents`|`3.0`|
|`smoothing`|`"normal"`|

## How it works

```
cpal WASAPI callback (20 Hz RBJ high-pass per channel) → SPSC ring
  → analyser thread, 512-sample hops at the device rate (~94 analyses/s)
  → level/gate → grid YIN + CMNDF → Goertzel harmonics → tracker → reading
```

- **Pitch**: grid YIN over `f_k = 1300·2^(−k/12)`, scanned ascending in lag so
  the smallest matching period wins (which is what stops bass from octave-up),
  then integer refinement and parabolic interpolation.
- **Two analysis windows**: 8192 samples (~171 ms) while acquiring and for any
  note below 55 Hz, 4096 (~85 ms) once locked above it. The state chip prints
  the window each reading actually used.
- **Capture** always uses the device's own default input config and default
  buffer size — never a forced sample rate, never a silent fallback to another
  interface. If the wrong device goes quiet, the window says so.

## Tests

```powershell
cargo test --release    # 44 tests (release: the DSP fixtures are seconds of audio)
cargo test --test dsp   # end-to-end analyser regressions only
```

Unit and integration tests regenerate their own audio in memory with the same
seeded generator `wavgen` uses, so a failing test can always be reproduced as a
WAV. They pin B0 lock and stillness, F1 accuracy, lock latency not growing with
pitch, no octave-up on a weak fundamental, a note holding its target while a
50 Hz hum sits within 10 dB of it, silence gating, drift tracking, and that
spelling flips change no numbers.

Not automated: live capture against real interfaces, config persistence, and UI
painting. Those are checked by hand per the protocol in
`TUNA_WINDOWS_TUNER_PLAN.md`; UI regressions are eyeballed from PNGs written by
`cargo build --release --features screenshot` plus `TUNA_SCREENSHOT_TO=shots/x.png`.

## Repository layout

|path|contents|
|---|---|
|`src/dsp/`|analyser pipeline: frame/level/gate, grid YIN, Goertzel, tracker|
|`src/audio/`|device enumeration, cpal stream + ring buffer worker, WAV reader/writer|
|`src/ui/`|note glyph, ±50 ¢ meter, chips, side panels, palette|
|`src/model/`|MIDI ↔ frequency ↔ names, tuning presets, config|
|`src/app.rs`|the only lifecycle owner: capture restarts, watchdogs, save|
|`src/cli.rs`, `src/bin/wavgen.rs`|flag parsing, headless run, test-signal generator|
|`tests/`|`dsp.rs` (analyser end-to-end), `model.rs` (presets/naming/spelling)|
|`TUNA_WINDOWS_TUNER_PLAN.md`|spec: algorithm choices, constants, acceptance criteria|
|`AGENTS.md`|contributor conventions: invariants, error style, test rules|

## License

MIT, as declared in `Cargo.toml`. There is no `LICENSE` file in the repo yet.
