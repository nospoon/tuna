# Repository Guidelines

## Project Overview

**Tuna** is a standalone Windows desktop guitar/bass tuner (`tuna` crate v0.1.0, edition 2021) shipped as a portable single `dist\tuna.exe` (no installer). Its hard requirement is reliable fundamental tracking at very low frequencies — bass B0 = 30.87 Hz, 8-string F1/F♯1 ≈ 43–46 Hz — with a display that never "floats" (no octave jumps, peak-to-peak < 3 ¢ when settled), and first-try capture from USB interfaces (Focusrite Scarlett 2i2, NUX Mighty Plug Pro) via cpal/WASAPI.

`TUNA_WINDOWS_TUNER_PLAN.md` (323 lines) is the authoritative spec: algorithm choices, constants, module map, acceptance criteria, and contingency plans. Read it before changing DSP, capture, or persistence behavior — it names the five "success-deciding" regions and what to do when acceptance fails. The repo is not yet a git repository; `.gitignore` covers `target/ dist/ fixtures/ shots/`.

## Architecture & Data Flow

Single crate, three targets: lib `tuna` (`src/lib.rs` — all logic, `pub mod app; audio; cli; dsp; model; synth; ui;`), bin `tuna` (`src/main.rs`), bin `wavgen` (`src/bin/wavgen.rs`). Integration tests reach modules only through the lib target.

Layering rule (enforced in `src/ui/mod.rs` docs): `audio → dsp → model` are pure logic; the UI sees plain values and "never touches cpal, the analyzer or the config file"; `app.rs` is the sole lifecycle owner. `cli.rs` reuses the exact live `Analyzer`/`HighPassBank` chain "so they cannot diverge."

Live signal flow (one shared `Analyzer`, sync threads only — no async runtime):

```
cpal WASAPI callback (rt: no locks/allocs)
  → HighPassBank::process every channel (20 Hz RBJ biquad, per-channel state)
  → rtrb SPSC ring (RING_CAPACITY 65_536 f32)
  → thread "tuna-analyser": pop HOP=512 → Analyzer::process_hop
      (level/gate → grid-YIN pitch → Goertzel harmonics → Tracker)
  → Reading published in audio::capture::Shared (mutexes + AtomicU64)
  → UI thread polls in TunaApp::ui(), request_repaint throttled to 16 ms
```

Offline path is identical: `--wav file --csv` → `Wav::read` → same HP + `HOP` chunking → same `Analyzer` → `Vec<Reading>`. `tests/dsp.rs::analyse()` mirrors it verbatim.

Key algorithm invariants (do not "optimize away"):
- **Pitch** (`src/dsp/pitch.rs`): grid YIN + CMNDF, coarse grid `f_k = 1300·2^(−k/12)` scanned **ascending τ — smallest τ wins** (reversing this octave-ups bass). No FFT crate; integer-τ refinement + parabolic interpolation. No autocorrelation-of-FFT, no HPS; harmonics use Goertzel only.
- **Tracker** (`src/dsp/track.rs`): 9–10 numbered anti-floating rules (gate → clarity freeze → median-3 → lock confirm → ±120 ¢ outlier clamp → EMA smoothing → hysteresis). States `Idle|NoSignal|Acquiring|Unstable|Locked`; hints `LowSignal|Clipping|WeakFundamental{strongest_h}|WrongPreset{semitones}`.
- **Capture** (`src/audio/device.rs`): always `device.default_input_config()` + `BufferSize::Default`; never `with_max_sample_rate()`, never hand-rolled `StreamConfig`, never guess a sample format (`build_input_stream_raw` + `FromSample`). Stall watchdog 600 ms; never auto-falls-back to another interface.
- **Spelling is display-only**: `Spelling{Sharp,Flat}` may be read only by `notes.rs` and painters; toggling must not change a single `f32` (enforced by `tests/model.rs::spelling_never_touches_a_single_number`).

State management: `TunaApp` owns `AppConfig` + `Arc<Shared>` + `Option<Capture>`; analyzer params change → full stream rebuild guarded by `Sig` fingerprint equality + `RESTART_DEBOUNCE`. Painters (`ui/panels.rs`, `ui/tuner_view.rs`) are pure fns over borrowed `Controls`/`View`, returning `Changed{capture,config,preset}` or a clicked `TargetMode`.

## Key Directories

| Path | Purpose |
|---|---|
| `src/dsp/` | `frame.rs` (Analyzer + all pipeline constants), `pitch.rs` (grid YIN), `track.rs` (state machine/`Reading`), `biquad.rs` (HP), `harmonics.rs` (Goertzel) |
| `src/audio/` | `device.rs` (enumerate/open), `capture.rs` (cpal stream, ring, worker), `wav.rs` (hand-rolled RIFF reader/writer) |
| `src/ui/` | `tuner_view.rs` (note glyph, ±50 ¢ meter, chips), `panels.rs` (top bar, IO side panel), `theme.rs` (palette consts), `mod.rs` (`TargetMode`, glyph fallback) |
| `src/model/` | `notes.rs` (MIDI↔freq↔names), `presets.rs` (15 presets), `config.rs` (`AppConfig`, serde) |
| `src/bin/` | `wavgen.rs` — deterministic test-signal WAV generator |
| `tests/` | `dsp.rs` (9 end-to-end DSP tests), `model.rs` (3 preset/naming/spelling tests) |
| `fixtures/` | gitignored generated WAV/CSV artifacts; regenerate with `wavgen`, never hand-edit |
| `dist/`, `shots/` | build outputs and screenshot verification PNGs (both gitignored) |

## Development Commands

All from `D:\tuna`; PowerShell for `build.ps1`.

```powershell
cargo build --release                      # dev build; dev profile has opt-level = 1
cargo test                                 # all 42 tests
cargo test --release                       # REQUIRED for DSP tests (93.8 analyses/s over multi-second fixtures)
cargo test --test dsp                      # integration tests only (also --test model)
cargo run --release -- --list-devices      # verify capture stack / device ids
cargo run --release -- --list-presets
cargo run --release -- --wav fixtures/b0.wav --preset bass_5 --csv   # offline headless run
cargo run --release -- --csv --seconds 10  # headless live capture
pwsh ./build.ps1                           # → dist\tuna.exe + dist\tuna-win-x64.zip
```

Generate fixtures (deterministic, seeded):

```powershell
cargo run --release --bin wavgen -- --out fixtures/b0.wav --midi 23 --seconds 6 --onset 300 --harmonics 8 --noise -60 --seed 7
cargo run --release --bin wavgen -- --out fixtures/f1.wav --midi 29 --seconds 6 --onset 300 --harmonics 8 --noise -60 --seed 11
```

Screenshot verification (own feature, **not** eframe's): `cargo build --release --features screenshot`, then `$env:TUNA_SCREENSHOT_TO="shots/x.png"; cargo run --release -- --wav fixtures/f1.wav --preset guitar_8_f` — writes at frame 12 so the font atlas has settled. CSV schema fixed by `cli::CSV_HEADER`: `t_ms,state,f_hz,cents,clarity,target,rms_db,gate_db`.

## Code Conventions & Common Patterns

- **Errors**: no anyhow/thiserror/clap/rand/ring — everything fallible returns `Result<T, String>` or collects `Vec<String>` (`Flags.errors`, `inventory(&mut diagnostics)`); binaries `exit(2)` for usage errors, `exit(1)` for failures. Every `Mutex` lock uses `unwrap_or_else(|e| e.into_inner())` (poison-tolerant). No `unsafe` anywhere.
- **CLI**: hand-rolled parsers (`cli.rs`, `bin/wavgen.rs`); accept both `--k v` and `--k=v`; USAGE string is the flag table.
- **Performance**: no allocation in the DSP/capture path (buffers sized once in `new`, `copy_within` history instead of ring math); `f32` throughout the audio chain (`f64` only in `synth.rs`); `#[inline]`/`#[inline(always)]` on hot kernels with a "let LLVM vectorise it" comment; digit separators (`48_000`, `65_536`).
- **Constants**: `SCREAMING_SNAKE` consts live in the owning module and are imported, never re-derived — pipeline knobs in `src/dsp/frame.rs` (`N_MAX`, `N_TRACK`, `HOP`, `YIN_THRESHOLD`, `CLARITY_ACCEPT`, `OUTLIER_CENTS`, gate/floor values). Changing one requires re-checking the plan's invariants (e.g. `W + τ_max ≤ N_MAX`).
- **Naming**: `snake_case`/`PascalCase`/`SCREAMING_SNAKE`; getters `foo()` / setters `set_foo(&mut self, …)`; state enums carry `name() -> &'static str` for CSV/labels; helpers private, only cross-module items `pub`; **no `pub use` re-exports** — consumers write full paths (`use tuna::dsp::frame::Analyzer;`). Preset ids exact: `chromatic`, `guitar_standard`, `guitar_8_fs`, `guitar_8_f`, `bass_5`, `custom`…
- **Docs**: `//!` module headers state *why* (invariants, bug histories, numeric rationale in ms/dB/cents/samples); pipeline steps numbered/lettered in comments. Match this when editing.
- **Serde**: `#[serde(default)]` on `AppConfig` — never add `deny_unknown_fields`; `rename_all = "lowercase"` enums match CLI vocabulary (`--spelling sharp|flat`); save via `config.toml.tmp` + rename to `%APPDATA%\tuna\config.toml`.
- **UI**: `egui` 0.37 pinned — expect API churn (e.g. `Align2::LEFT_BASELINE` removed, `StrokeKind` explicit); fix against 0.37 docs, never downgrade. Colors only via `ui/theme.rs` consts over `Visuals::dark()`; glyph fallback ♯/♭ → `#`/`b` handled by `ui::spell`. Unicode as escapes (`\u{266F}`), typography uses U+2212/U+2014/U+00A2.
- **Never relax DSP test thresholds** to make a change pass — the plan's contingencies (e.g. `YIN_THRESHOLD = 0.15` + extra confirm frames, median-5) are the sanctioned levers, and octave errors should surface as honest `Unstable`.

## Important Files

- `Cargo.toml` — targets, `screenshot = ["dep:png"]` feature, profiles (dev `opt-level=1`; release `lto="fat"`, `codegen-units=1`, `strip=true`).
- `TUNA_WINDOWS_TUNER_PLAN.md` — spec + verification protocol + contingencies.
- `src/main.rs` — entry point: CLI → config load → offline/live/headless/GUI dispatch; `Renderer::Glow` (not wgpu, so software GL fallback works).
- `src/app.rs` — `TunaApp`: lifecycle, capture restart logic, watchdogs, debounced save, screenshot hook.
- `src/dsp/frame.rs` | `pitch.rs` | `track.rs` | `src/audio/device.rs` | `src/model/notes.rs` | `src/ui/tuner_view.rs` — the five+ success-deciding regions called out by the plan.
- `src/synth.rs` — the single signal generator shared by `wavgen`, fixtures, and tests; keep deterministic (own LCG, distinct seeds).

## Runtime/Tooling Preferences

- **Rust only**, host `x86_64-pc-windows-msvc`; empirically Rust 1.99.1 + VS 2019 BuildTools (MSVC 14.29) + Windows SDK 10.0.19041. No `rust-toolchain.toml` pin, no CI, no `.cargo/config.toml`. `src/ui/panels.rs` uses `u32::is_multiple_of` → keep rustc recent despite edition 2021.
- eframe/egui **0.37 with `glow`** (no wgpu, no eframe `persistence` — config is hand-written), cpal 0.18 (WASAPI), rtrb, serde+toml+dirs, optional png. No other deps are sanctioned; adding one (FFT, clap, rand) contradicts the plan.
- Build/packaging via `build.ps1` (PowerShell 5+, uses `Compress-Archive`); zip pattern `tuna-<platform>-<arch>.zip`.
- `shots/pnglib.py` is a stray ad-hoc PNG inspector from earlier visual checks; not part of the build.

## Testing & QA

Plain `std` `#[test]`, zero dev-dependencies, 42 tests total:

- **Inline** `#[cfg(test)] mod tests` in nearly every module (30 tests): units close to the math — biquad, gate/floor, pitch grid (`grid_is_ascending_lag` pins exact τ bounds), tracker rules, config, presets, notes, CLI parsing, WAV round-trip, synth determinism.
- **Integration** `tests/dsp.rs` (9): end-to-end `Analyzer` regressions — every fixture "is one the tuner used to get wrong": B0 lock/stillness, F1 accuracy, lock latency not growing with pitch, weak-fundamental no-octave-up, hum immunity, WrongPreset hint, silence gating, sinusoidal drift tracking.
- **Integration** `tests/model.rs` (3): 8-string presets, low-note naming, bit-identical numbers under spelling flip.

Tests never read `fixtures/` — they regenerate the same audio in memory via `tuna::synth::render(&SynthParams)` (seeded, `fs 48_000`, hard-coded MIDI literals), so any failing test is reproducible as a WAV via `wavgen`. Test names are lowercase English sentences stating the invariant (`b0_locks_fast_and_sits_still`, `spelling_never_touches_a_single_number`); every assert embeds the measured number and its budget in the failure message. Never loosen thresholds or add ignore flags.

Uncovered by automation (verified manually per plan §Verification): live capture (`audio/capture.rs`, `device.rs` — the real acceptance test is plugging in a Scarlett and playing), `app.rs` persistence, and UI painting — the latter checked via `screenshot`-feature PNGs in `shots/`. Run `cargo test --release` plus one `--wav … --csv` headless run as the minimum proof after touching DSP or capture code.
