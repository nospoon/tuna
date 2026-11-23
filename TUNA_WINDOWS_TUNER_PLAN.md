# Tuna — standalone Windows tuner (bass / 8-string grade)

## Context

Build a from-scratch Windows desktop tuner in `D:/tuna` (currently empty, not a git repo). It must track fundamentals reliably at low frequencies — bass low B0 = 30.87 Hz, 8-string F♯ standard F♯1 = 46.25 Hz, 8-string F standard F1 = 43.65 Hz — with a display that does not float (no octave jumps, no needle hunting), must capture correctly from USB interfaces (Focusrite Scarlett 2i2, NUX Mighty Plug Pro), and must present a simple modern display that is easy to read at a glance.

Verified on this machine: Rust 1.99.1 / cargo, default host `x86_64-pc-windows-msvc`, VS 2019 BuildTools (`VC\Tools\MSVC\14.29.30133`, `vcvars64.bat`) + Windows SDK `10.0.19041.0` → MSVC linking works; crates.io reachable; `Focusrite USB Audio` present in `Win32_SoundDevice`, plus `USB Audio Device` (the NUX class), `VB-Audio Voicemeeter VAIO`, `Steam Streaming Speakers`. No .NET SDK, no CMake — stack is fixed to Rust.

Stack (decided): Rust + `eframe`/`egui` 0.37 with `glow` renderer, custom-painted **linear ±50 ¢ meter** as the primary readout, `cpal` 0.18 WASAPI capture, portable single `tuna.exe`.

## Approach

Ordered so each step compiles and its own tests pass before the next. Steps 2–3 (model + DSP) have no audio dependency and are testable headlessly; step 4 is the live capture path; steps 5–7 are UI and persistence.

### 1. Project scaffold

`Cargo.toml` (edition 2021). Pinned deps, all confirmed to exist on crates.io:

```toml
[dependencies]
cpal = "0.18.2"                       # WASAPI capture on Windows
egui = "0.37.0"
eframe = { version = "0.37.0", default-features = false,
           features = ["default_fonts", "system_fonts", "glow", "clipboard", "links"] }
rtrb = "0.4.1"                        # SPSC ring buffer (lock-free, no alloc after new())
serde = { version = "1.0.229", features = ["derive"] }
toml = "1.2.2"
dirs = "7.0.0"

[profile.dev]
opt-level = 1                          # so live capture keeps up in a debug build

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
```

No `wgpu` (glow only → much smaller binary/build), no `persistence` (config is written by us), no FFT crate: the pitch algorithm below is correlation-based and needs no FFT. `system_fonts` must stay enabled — it is how `♯` (U+266F) / `♭` (U+266D) get a Segoe UI Symbol fallback, since egui's bundled fonts lack them.

`src/main.rs` for now: parse CLI flags (spec in step 5), then `eframe::run_native("Tuna", native_options, Box::new(|cc| Ok(Box::new(app::TunaApp::new(cc, &flags)))))` with `NativeOptions { inner_size: Some(Vec2::new(1000.0, 620.0)), min_inner_size: Some(Vec2::new(760.0, 480.0)), decorated: true, ..Default::default() }`. `eframe::App` in 0.37 requires **`fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`** (there is no `update`); non-painting work goes in `fn logic(&mut self, ctx, frame)`.

Add `build.ps1` at repo root: `cargo build --release`, copy `target\release\tuna.exe` to `dist\tuna.exe`, Compress-Archive to `dist\tuna-win-x64.zip`. No installer.

### 2. Note / tuning / spelling model — `src/model/notes.rs`, `src/model/presets.rs`

Pure module, zero deps, write its tests first (they are listed in Verification and are cheap).

```rust
pub const PITCH_FLOOR_HZ: f32 = 27.5;   // A0 = MIDI 21; see step 3 for why this is the search floor
pub const PITCH_CEIL_HZ:  f32 = 1300.0; // ~G#6/A6, above the highest string of any preset
pub const A4_DEFAULT: f32 = 440.0;

pub fn midi_to_freq(m: u8, a4: f32) -> f32;   // a4 * 2^((m as f32 - 69.0) / 12.0)
pub fn freq_to_midi(f: f32, a4: f32) -> f32;  // 69.0 + 12.0 * log2(f / a4)
pub fn cents_between(f_meas: f32, f_target: f32) -> f32; // 1200.0 * log2(f_meas / f_target)

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Spelling { Sharp, Flat }

pub fn note_name(m: u8, spelling: Spelling) -> String;
```

`note_name`: octave = `(m / 12) - 1` (scientific: C4 = MIDI 60; MIDI 29 → `F1`, MIDI 34 → `Bb1`/`A#1`, MIDI 23 → `B0`). Letter tables, index = `m % 12`:

- `SHARP_NAMES = ["C","C♯","D","D♯","E","F","F♯","G","G♯","A","A♯","B"]`
- `FLAT_NAMES  = ["C","D♭","D","E♭","E","F","G♭","G","A♭","A","B♭","B"]`

Return e.g. `format!("{letter}{octave}")`. The glyphs are a single pair of `&str` consts so a missing-glyph problem is a one-line change to `"#"`/`"b"` (checked in Verification with a screenshot).

`presets.rs`:

```rust
pub struct Preset { pub id: &'static str, pub label: &'static str,
                    pub midi: &'static [u8], pub spelling: Spelling }
pub const PRESETS: &[Preset] = &[ ... ];
pub fn preset_by_id(id: &str) -> Option<&'static Preset>;
```

Exact table (MIDI numbers, low→high string):

| id | label | midi | suggested spelling |
|---|---|---|---|
| `chromatic` | Chromatic | `&[]` | Sharp |
| `guitar_standard` | Guitar standard E | 40 45 50 55 59 64 | Sharp |
| `guitar_drop_d` | Guitar drop D | 38 45 50 55 59 64 | Sharp |
| `guitar_dadgad` | DADGAD | 38 45 50 55 57 62 | Sharp |
| `guitar_open_g` | Open G | 38 43 50 55 59 62 | Sharp |
| `guitar_half_step` | Half step down (E♭) | 39 44 49 54 58 63 | **Flat** |
| `guitar_drop_c` | Drop C | 36 43 48 53 57 60 | Sharp |
| `guitar_7_low_b` | 7-string low B | 35 40 45 50 55 59 64 | Sharp |
| `guitar_8_fs` | 8-string F♯ standard | 30 35 40 45 50 55 59 64 | Sharp |
| `guitar_8_f` | 8-string F standard | 29 34 39 44 49 54 58 63 | **Flat** |
| `bass_4` | Bass 4-string E | 28 33 38 43 | Sharp |
| `bass_4_drop_d` | Bass drop D | 26 33 38 43 | Sharp |
| `bass_5` | Bass 5-string B | 23 28 33 38 43 | Sharp |
| `bass_6` | Bass 6-string B | 23 28 33 38 43 48 | Sharp |
| `custom` | Custom | from config | user's current |

Lowest string in the whole table is MIDI 23 = 30.87 Hz (5/6-string bass B0); 8-string F standard's F1 = `midi_to_freq(29, 440)` = 43.65 Hz. Both sit above `PITCH_FLOOR_HZ = 27.5` (A0) with ≥ 3 semitones of margin, which is what makes the search-range sizing in step 3 valid for these tunings.

**Spelling is display-only.** `cents_between`, `midi_to_freq`, `freq_to_midi`, and every DSP value are computed with MIDI numbers only; `Spelling` is consumed exclusively by `note_name` and by the string-chip labels. Toggling it must not touch a single `f32`.

**Spelling switching rule (implement exactly):** `TunaApp.spelling: Spelling` lives in the app state and is persisted. (a) Startup: config value wins. (b) When the user picks a preset (combo box, or `--preset`), set `self.spelling = preset.spelling` — that is how `guitar_8_f` and `guitar_half_step` come up in flats. (c) Clicking the ♯/♭ toggle overrides it immediately and persists. (d) Re-picking a preset re-applies that preset's suggestion. A manual override therefore survives across restarts until a preset is re-picked.
### 3. DSP core — `src/dsp/{frame.rs,pitch.rs,harmonics.rs,track.rs}`

One shared unit that **both** the live-capture path and the offline WAV path feed, so headless tests exercise the real chain:

```rust
pub struct Analyzer { /* fs, biquad state, hist [f32; HOP+N_MAX], rms/peak/floor state,
                        Tracker, coarse+fine scratch buffers, hann table — all pre-allocated */ }
impl Analyzer {
    /// One hop of mono, *unfiltered* device samples. Returns None when a hop is not yet
    /// analysed (warm-up) or is below gate.
    pub fn process_hop(&mut self, hop: &[f32]) -> Option<Reading>;
    pub fn set_channel(&mut self, ch: usize);   // atomic-ish switch, glitch-free (see below)
    pub fn set_a4(&mut self, a4: f32);          // retargets note names/cents only
}

pub struct Reading {
    pub t_ms: u64,               // ms since analyzer start
    pub f_hz: f32,               // smoothed frequency, derived from smoothed cents (see rule below)
    pub cents: f32,              // smoothed cents vs. current target centre
    pub clarity: f32,            // 0..1 confidence of the newest raw estimate
    pub state: TrackState,       // Idle | NoSignal | Acquiring | Unstable | Locked
    pub target_midi: Option<u8>,
    pub rms_db: f32, pub peak_db: f32, pub gate_db: f32, pub clipped: bool,
    pub harmonic_amps: [f32; 12],// h=1..12, normalised so max == 1.0
    pub hint: Option<Hint>,      // LowSignal | Clipping | WeakFundamental{strongest_h} | WrongPreset{semitones}
}
```

**Geometry (fixed constants, in `dsp/frame.rs`):** `N_MAX = 8192`, `HOP = 512` samples (10.7 ms at 48 kHz → 93.8 analyses/s), `YIN_THRESHOLD = 0.10`, `CLARITY_ACCEPT = 0.45`, `CLARITY_GOOD = 0.70`, `LOCK_CONFIRM = 3` frames, `NOTE_CHANGE_CONFIRM = 6` frames, `OUTLIER_CENTS = 120.0`, `GATE_AUTO_MARGIN_DB = 10.0`, `GATE_HYSTERESIS_DB = 4.0`, `NOISE_HOLD_MS = 500`.

`hist` is a 8192-sample linear buffer with `copy_within(HOP.., 0)` then the new hop appended; analysis reads the tail only. `copy_within` at 93.8 hops/s ≈ 3 MB/s of memcpy — irrelevant, and it keeps the code allocation-free and trivially correct.

**a. High-pass, run per channel inside the capture callback** (`biquad.rs`, Direct-Form I, RBJ cookbook, state kept per channel index 0..7 so switching the selected input channel mid-stream is glitch-free — every channel is filtered every block whether or not it is displayed):
`fc = 20.0 Hz`, `Q = 0.7071`, coefficients recomputed only when `fs` or `fc` change:
`w0 = 2π·fc/fs`, `α = sin(w0)/(2Q)`, `b0 = (1+cos w0)/2`, `b1 = -(1+cos w0)`, `b2 = b0`, `a0 = 1+α`, `a1 = -2·cos w0`, `a2 = 1-α`; divide all by `a0`. Removes DC rumble and sub-audible interface noise without touching F1/B0 magnitude (at 30 Hz the 20 Hz HP costs < 0.5 dB).

**b. Level + gate** (per hop, on the selected channel's filtered samples): `peak = max|x|`, `rms = sqrt(Σx²/HOP)`, dBFS with `-160.0` floor for silence. Smooth RMS with `α = 1 - exp(-hop_dur/0.050)`. Noise floor tracker: `if rms_db < floor_db { floor_db = rms_db } else { floor_db = min(rms_db, floor_db + 0.05) }` (follows down instantly, recovers upward at 0.05 dB/hop ≈ 4.7 dB/s). Gate open level `gate_db = if auto_gate { max(floor_db + GATE_AUTO_MARGIN_DB, user_floor_db) } else { user_floor_db }` with `user_floor_db` default `-60.0`; gate opens at `gate_db`, closes below `gate_db - 4.0` (hysteresis). `clipped = any(|x| ≥ 0.999)`, held 1 s.

**c. Pitch — grid YIN + local refinement** (`pitch.rs`). Difference function on the frame tail of length `N`: `d(τ) = Σ_{n<W} (x[n] − x[n+τ])²`, with

- acquisition (`state ∈ {Idle, NoSignal, Acquiring, Unstable}`): `N = 8192`, `W = 4096`. Constraint `W + τ_max ≤ N` holds: `τ_max = fs/27.5 = 1745` @48 kHz (`1601` @44.1 kHz) → 5841 ≤ 8192. At the floor, `W` spans 2.35 periods; at B0 it spans 2.62 — enough correlation baseline for a unique period.
- locked tracking: `N = if f_center >= 55.0 { 4096 } else { 8192 }`, `W = clamp(3·τ_center, 1024, N/2)`. Display latency is set by `N`: ~171 ms below 55 Hz, ~85 ms above. That is the correct physics — 30 Hz cannot be resolved faster than a few periods — and is faster than hardware tuners at B0 (~300 ms).

Coarse candidate grid, **ascending τ** (= descending frequency, which is YIN's scan direction and the octave-error guard): `f_k = 1300.0 · 2^(−k/12)` for `k = 0..=66` (68 points, 100 ¢ apart), `τ_k = (fs / f_k).round() as usize`, clamped into `[τ_min, τ_max]`, duplicates dropped. Cumulative-mean normalisation `d′(k) = d_k / ((1/(k+1))·Σ_{j≤k} d_j)` with `d′(0) = 1.0`. Scan `k` ascending and take the **first local minimum with `d′ < YIN_THRESHOLD`** — this "smallest τ wins" rule is precisely what stops octave-up errors from harmonic-rich bass signals. If none passes, fall back to the global minimum of `d′` only if `d′ < 0.25` (reported as low clarity); otherwise return `clarity = 0.0` (unvoiced).

Refinement, because the 100-cent coarse grid is far coarser than a cent: `Δ = max(3, ceil(0.0293 · τ_k) + 1)` (0.0293 = `2^(50/1200) − 1`, i.e. the worst-case distance from a coarse grid point), then evaluate plain `d(τ)` at **every integer τ** in `[τ_k − Δ, τ_k + Δ]`, take the minimum, parabolic-interpolate: `δ = 0.5·(d₋ − d₊)/(d₋ − 2d₀ + d₊)` (skip if `|denominator| < 1e-12`), `f_raw = fs / (τ̂ + δ)`. Grid cost ≈ 68·4096 + 105·4096 ≈ 7·10⁵ MACs per analysis ≈ 66 M MAC/s at 93.8 hops/s — a few percent of one core. No allocation anywhere in this path; `f32` accumulation, plain indexed loops over contiguous `&[f32]` so LLVM auto-vectorises.

`clarity = (1.0 − d′_chosen).clamp(0.0, 1.0)`.

**d. Harmonic diagnostic** (`harmonics.rs`): Goertzel magnitude at `h·f_display` for `h = 1..=12` on the same frame tail, Hann-windowed into a scratch buffer (table precomputed per `N`). Normalise by the max harmonic; if `amp[0] < 0.25 · max` set `Hint::WeakFundamental { strongest_h }`. Goertzel avoids pulling in an FFT for a 12-bar strip.

**e. Tracker** (`track.rs`) — this is where "no floating" comes from. Order exactly:

1. If the gate is closed: `NoSignal`; the UI keeps the last reading dimmed for `NOISE_HOLD_MS`, then clears.
2. `clarity < CLARITY_ACCEPT` → `Unstable`; **freeze the smoother** (the needle stops moving instead of sliding toward noise — the single most important anti-floating rule).
3. 3-point median of `f_raw` (shift register).
4. Not yet locked: `Acquiring`; lock when 3 consecutive medians agree within ±35 ¢ of each other; `f_center = median3`, `target_midi = freq_to_midi(f_center, a4).round()` clamped to `midi_to_freq ∈ [PITCH_FLOOR_HZ, PITCH_CEIL_HZ]`, or — pinned-string mode — `target_midi = preset.midi[i]`.
5. Locked: `outlier = |1200·log2(f_raw / f_center_target)| > OUTLIER_CENTS` where `f_center_target = midi_to_freq(target_midi, a4)`. While ≤ `NOTE_CHANGE_CONFIRM − 1` outliers occur, **clamp** `f_raw` to the ±120 ¢ window (garbage cannot drag the needle). On `NOTE_CHANGE_CONFIRM` consecutive outliers: if the outlier's nearest note differs from `target_midi`, switch `target_midi` and reset the smoother to the fresh median (so a deliberate string change snaps instead of sweeping); if it is the same note (i.e. a genuinely detuned string), re-centre without changing the note name.
6. `cents_raw = cents_between(f_raw, midi_to_freq(target_midi, a4))` → 3-point median → EMA `α = 1 − exp(−hop_dur/τ_smooth)`, `τ_smooth` = Fast 60 ms / Normal 140 ms / Slow 300 ms. Reset the EMA state on lock onset, note change, and gate reopen.
7. `f_hz = midi_to_freq(target_midi, a4) · 2^(cents_smoothed/1200)` — **Hz and cents derive from the one smoothed value**, so the two readouts can never contradict each other.
8. `InTune` (drives the green state, computed in the UI from `|cents| ≤ tolerance`): true after 2 consecutive frames inside, drops out only at `|cents| > tolerance + 3.0` (hysteresis so the colour does not flicker at the threshold).
9. `WrongPreset { semitones }` when `|freq_to_midi(f_raw) − target_midi| ≥ 1.5` — the hint line then says the measured pitch is N semitones off the selected tuning.
10. Harmonic strip and hints are computed from the frozen/locked `f_display`, never from a rejected outlier.

### 4. Live capture — `src/audio/{device.rs,capture.rs}`

- Host: `cpal::default_host()` (that is WASAPI on Windows; `DeviceId` strings look like `wasapi:{...}`). Enumerate with `host.input_devices()`, and per device call `description()` (for the display name) and `id()` (`cpal::DeviceId`, `Display`/`FromStr`-serialisable, `host.device_by_id(&id)` for restore). Keep only `supports_input()` devices.
- Format: **`device.default_input_config()?` → `.config()` + `.sample_format()`**, with `BufferSize::Default`. That is the WASAPI shared-mode mix format and the only choice guaranteed to activate on both the Scarlett (48 kHz / 2 ch / f32) and the NUX class device. Never call `with_max_sample_rate()` or build a hand-rolled `StreamConfig`: in shared mode the rate must equal the endpoint mix rate or activation fails. If `default_input_config()` errors (device unplugged/busy), do not guess a format — mark that device unusable, push the cpal error text into the diagnostics list, and fall back to `host.default_input_device()`.
- Stream: `device.build_input_stream(config, callback, err_callback, Some(Duration::from_millis(1000)))` with typed `f32` callback when `sample_format == SampleFormat::F32`; for `I16 | I32 | I24 | F64` use `build_input_stream_raw(config, sample_format, |data: &cpal::Data, _| ...)` and convert with `data.as_slice::<T>()` (cpal 0.18 `Data` has only `as_slice`/`as_slice_mut`, no converting accessor) plus `f32::from_sample` via `cpal::{Sample, FromSample}`; unknown format → error surfaced, not silently treated as silence. `BufferSize::Default` — never force a size (shared mode does not honour it usefully).
- Callback (real-time thread, zero allocation on the f32 path): de-interleave `channels` frames, filter each channel through its own biquad, push the **selected** channel (index read from an `AtomicUsize`) into `rtrb::Producer<f32>` with `push_partial_slice`; on `PushError::Full`, drop the oldest by `consumer.pop_partial_slice` of the overflow length and retry once. Ring capacity 65 536 samples (1.36 s @48 kHz).
- Worker thread `dsp`: `loop { pop up to HOP samples into a scratch hop; if a full hop was read → analyzer.process_hop(); publish }`. Publish = `Mutex<Option<Reading>>` swap (poison-tolerant: `unwrap_or_else(|e| e.into_inner())`) plus `ctx.request_repaint()` throttled to one per 16 ms via `Instant`. When fewer than `HOP` samples are available, `std::thread::sleep(Duration::from_millis(3))` — no busy spin, no timed waits on the audio thread.
- Recovery, driven from `App::logic` every 250 ms: the cpal error callback and the stream-creation result set `AudioStatus::Failed(String)`; on failure, drop the stream, `sleep(250 ms)` and rebuild. Every 2 s re-enumerate devices: if the selected `DeviceId` is gone, switch to `host.default_input_device()` and show `Disconnected`; if it returns, do **not** auto-switch back — the user picks it. Dropping the `cpal::Stream` is the only teardown (its `Drop` stops the callback).

### 5. Offline signal harness — `src/bin/wavgen.rs` + `--wav` path in `src/audio/wav.rs`

This is what makes the low-frequency claims verifiable without a guitar, and it feeds the exact same `Analyzer` from step 3 (it bypasses only cpal and the ring buffer).

`wavgen.rs` writes 16-bit PCM mono WAVs at 48 kHz, deterministic via `--seed`, with a pluck envelope (5 ms attack, exponential decay `τ = 0.6 s`, tail to silence) and `--onset <ms>` of silence before it:

```
cargo run --release --bin wavgen -- --out fixtures/b0.wav --midi 23 --seconds 6 \
    --onset 300 --harmonics 8 --rolloff 1.0 --noise -60 --seed 7
```

Flags: `--out`, `--midi`, `--seconds`, `--fs`, `--onset`, `--harmonics`, `--rolloff` (harmonic amplitude `1/h^rolloff`), `--noise <dBFS>` (white, added after the envelope), `--hum <dBFS>` (sine at 50 Hz), `--fundamental-scale <0..1>` (attenuate h=1 only — the octave-error stress), `--drift <cents>` (slow sinusoidal detune), `--seed`.

`wav.rs`: minimal reader for 16-bit PCM and 32-bit float, mono or interleaved, any rate (canonical 44-byte header walk; unsupported file → clear message on stderr, exit 2).

CLI, parsed by hand in `main.rs` (no clap): `--list-devices` (per input device: name, `DeviceId`, `default_input_config` summary — the check for step 4), `--device <substring>`, `--channel <n>`, `--preset <id>`, `--spelling sharp|flat`, `--a4 <hz>`, `--wav <path>` (offline: run the whole file through `Analyzer`, keep only the final `Reading`, never start cpal), `--csv` (print `t_ms,state,f_hz,cents,clarity,target,rms_db,gate_db` per analysis and exit; with `--wav` it is offline, without it it runs headless on live capture for `--seconds <n>`).


### 6. UI — `src/app.rs`, `src/ui/{theme.rs,tuner_view.rs,panels.rs}`

**Theme** (`theme.rs`): in `TunaApp::new`, `cc.egui_ctx.set_visuals(egui::Visuals::dark())`; do not hand-edit `Visuals` fields (they churn between egui releases). All colour decisions live in `theme.rs` as `Color32` consts and are applied explicitly to custom painting, `egui::Frame::new().fill(…)`, and `RichText::…color(…)`: `BG = #101014`, `SURFACE = #17171D`, `BORDER = #2A2A33`, `FG = #E6E6EC`, `MUTED = #8C8C99`, `ACCENT = #4CC2FF`, `SUCCESS = #3FD37A`, `WARNING = #F0A63C`, `ERROR = #E8545E`.

**Layout**, inside `fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame)`: `egui::TopBottomPanel::top("top_bar")`, then `egui::SidePanel::left("io_panel").exact_width(250.0)`, then `egui::CentralPanel::default()`; each shown through `egui::Frame::new().fill(SURFACE).corner_radius(6.0).inner_margin(12.0)`.

**Top bar**, left→right: device `egui::ComboBox::from_id_salt("device")` (red caption line under it whenever `AudioStatus != Running`), input-channel combo (`Input 1`, `Input 2` — count from the live stream config), format label `48 000 Hz · 2 ch · f32`, the **♯/♭ toggle** below, `±1¢ ±3¢ ±5¢` tolerance `SelectableLabel`s, `Fast Normal Slow` smoothing, `A4` `DragValue` (435–455, step 0.5, format `%6.1 Hz`), preset combo.

**♯/♭ toggle is a first-class control, not a settings entry**: two adjacent `Button`s, `RichText::new("♯").size(26.0)` and `RichText::new("♭").size(26.0)`, 40 px tall, sitting right of the device picker; active one filled `ACCENT` with `BG` text, inactive `SURFACE`/`MUTED`. Click sets `self.spelling`, marks config dirty (debounced save, step 7), repaints. It re-spells the big note glyph, target chip, string chips and preset label only — `f_hz`, `cents` and needle geometry are untouched, because `Spelling` never reaches the DSP (step 2's rule).

**Left panel**: level meter — horizontal bar over `−80…0 dBFS`, ticks every 6 dB, fill `ACCENT`, peak-hold line, `MUTED` vertical marker at `gate_db` with caption `gate −52.4 dBFS · floor −63.1 dBFS`, `ERROR` square LED latched 1 s after a clip. Below it the **harmonic strip**: 12 bars, height = `Reading::harmonic_amps[h]` (h = 1…12, normalised to max = 1.0), x labels `1 2 3 … 12`, `MUTED` dashed reference at 0.25, h=1 bar tinted `WARNING` under `Hint::WeakFundamental`. Below that, the diagnostics list: last 12 status strings (`opened Focusrite USB Audio 48000 Hz f32 2ch`, `stream error: device unavailable — retrying`, …) so audio faults are visible in a GUI process where stderr is not.

**Central panel**, top→bottom:

1. **String chips row** (preset modes only): one small button per string labelled `F♯1` … `E4` in the current spelling, current target `ACCENT`-outlined; clicking pins that string as the target (`TargetMode::Pinned(i)` → the ±120 ¢ restricted search, the strongest anti-floating mode); an `Auto` chip returns to `TargetMode::Auto`. Row hidden in `chromatic`.
2. **Note glyph**: letter `FontId::proportional(150.0)`, accidental `96.0`, octave `72.0`, baseline-aligned via `Align2::LEFT_BASELINE` on a shared `baseline_y`. Colour: `SUCCESS` when in tune (hysteresis per step 3.8), `FG` when locked and off, 40 %-alpha `MUTED` during the `NOISE_HOLD_MS` hold, nothing in `Idle`. Right of it, `MUTED` 18 px: `target F♯1 · 46.25 Hz`.
3. **Cent meter** (`tuner_view.rs`, custom painted, min height 120 px, panel width): range `±50 ¢`; dead band `|c| ≤ tolerance` filled `SUCCESS` at 12 % alpha; centre line 2 px `FG`; minor ticks every 5 ¢, major every 10 ¢ labelled `−50 −25 0 +25 +50` with a trailing `¢`; `FLAT` / `SHARP` in `MUTED` at the ends. Needle = 5 px rounded bar, height 0.8·panel, `SUCCESS`/`WARNING` by state, 1 px `BG` outline so it reads over the dead band; `|cents| > 50` pins it to the edge and draws a chevron while the numeric readout keeps the true value (clamped ±1200 ¢).
4. **Readout row**, `FontId::monospace(30.0)` so digits cannot jitter horizontally: `41.20 Hz`, `−4.3 ¢`, and a state chip `Locked · 171 ms` / `Acquiring` / `Unstable` / `No signal` / `Disconnected`.
5. **Hint line**, 16 px `MUTED`, from `Reading::hint`: `Signal below gate (−62.4 dBFS) — raise interface gain` / `Clipping — lower interface gain` / `Fundamental weak: strongest harmonic h=3 — check input/impedance` / `Measured pitch is 12 semitones below F♯1 — wrong tuning selected?`.

**Repaint policy**: `ctx.request_repaint_after(Duration::from_millis(16))` while the gate is open, `250 ms` when idle. In `--wav` mode the offline pass must run **before the first paint**, so the window — and the screenshot check in Verification — shows the settled state.


### 7. Persistence + custom tuning — `src/model/config.rs`

Path `dirs::config_dir()/tuna/config.toml` (i.e. `%APPDATA%\tuna\config.toml`), created on demand.

```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub device_id: String,          // cpal DeviceId string, "" = system default input
    pub input_channel: usize,       // 0-based
    pub preset: String,             // preset id, or "custom" / "chromatic"
    pub custom_tuning: Vec<u8>,     // MIDI, low→high, validated 21..=108
    pub pinned_string: Option<usize>,
    pub a4_hz: f32,                 // 435.0..=455.0, default 440.0
    pub spelling: Spelling,         // "sharp" | "flat", default Sharp
    pub tolerance_cents: f32,       // 1.0 | 3.0 | 5.0, default 3.0
    pub smoothing: Smoothing,       // fast | normal | slow, default normal
    pub auto_gate: bool,            // default true
    pub gate_floor_db: f32,         // default -60.0, used when auto_gate = false and as auto clamp floor
}
```

Load: missing file → defaults; parse error → defaults **plus** rename the bad file to `config.toml.bak` and put the reason in the diagnostics list (never fail startup on config). Save: debounced — `App::logic` writes when `dirty && last_save.elapsed() > 500 ms`, via `config.toml.tmp` + `std::fs::rename` (atomic enough on NTFS). Any control change sets `dirty`. `--preset/--spelling/--a4/--channel/--device` flags override the file for that run without writing it back.

Both enums serialize lowercase: `#[derive(Serialize, Deserialize)] #[serde(rename_all = "lowercase")] pub enum Spelling { Sharp, Flat }` (defined in `notes.rs`) and `pub enum Smoothing { Fast, Normal, Slow }` (defined here in `config.rs`). So the file keys read exactly `spelling = "flat"` and `smoothing = "normal"`, matching the `--spelling sharp|flat` flag vocabulary.

Custom tuning editor (left panel section, visible when `preset == "custom"`): one row per string as `◄ F♯1 ►` — two small buttons plus the label from `note_name(midi, self.spelling)`, stepping that entry ±1 clamped to 21..=108 — then `+ add string` (appends MIDI 64) and `− remove` (drops the highest). Entries stay sorted ascending; a row whose neighbours collide is still legal (unison strings are valid). Everything routes through the same `note_name`/`Spelling` path, so the ♯/♭ toggle re-spells the editor too.

### Module map (final file list)

```
Cargo.toml  build.ps1
src/main.rs                 CLI parse, run_native, offline/headless branches
src/app.rs                  eframe::App: state, panels wiring, config save, recovery ticks
src/ui/theme.rs             Color32 consts, fonts, Visuals::dark() install
src/ui/tuner_view.rs        note glyph + cent meter painting
src/ui/panels.rs            top bar (device/channel/♯♭/tolerance/smoothing/A4/preset),
                            level meter, harmonic strip, diagnostics, string chips
src/model/notes.rs          MIDI↔freq↔name, cents math, Spelling
src/model/presets.rs        PRESETS table + preset_by_id
src/model/config.rs         AppConfig load/save/debounce
src/audio/device.rs         enumeration, default_input_config pick, rebuild/recovery
src/audio/capture.rs        cpal callback → biquad → mono extract → rtrb producer
src/audio/wav.rs            offline WAV source for --wav/--csv
src/dsp/biquad.rs           RBJ HP per channel
src/dsp/frame.rs            Analyzer: hop history, level/gate, hann, hop scheduling
src/dsp/pitch.rs            grid YIN + CMNDF + integer-τ refinement + clarity
src/dsp/harmonics.rs        Goertzel h=1..12
src/dsp/track.rs            median/clamp/EMA/lock state machine → Reading
src/bin/wavgen.rs           deterministic fixture generator
src/synth.rs                deterministic pluck/harmonic/noise/hum generator — used by wavgen AND tests
tests/dsp.rs                fixture-based end-to-end assertions (step-3 chain via Analyzer)
tests/model.rs              notes/spelling/preset-table assertions
fixtures/                   generated WAVs (git-ignored; regenerate with wavgen)
```

No `lib.rs`: `main.rs` + `#[cfg(test)]` unit tests inside each dsp/model module, plus the two integration test files which reach the same code through `cargo test` (make the crate a bin+lib split only if `tests/` cannot see the modules — if it can't, add `src/lib.rs` re-exporting `dsp`, `model`, `audio`; decide at the first `cargo test` run, do not restructure tests around it).


## Critical files & anchors

| file | region | why it decides success |
|---|---|---|
| `src/dsp/pitch.rs` | CMNDF scan + integer-τ refine | The octave-error locus: the grid **must** be generated ascending τ (descending f) and the chosen minimum must be the *first* one under `YIN_THRESHOLD`. Reverse the scan direction and the app octave-ups on harmonic-rich bass, which is exactly the failure being designed out. |
| `src/dsp/track.rs` | rules 2–6 in step 3 | "No floating" lives here: freeze-on-low-clarity, ±120 ¢ clamp, 6-frame note-change confirm, EMA reset. A tuner with good `f_raw` and a bad tracker still flickers. |
| `src/audio/device.rs` | config pick + recovery | Interface passthrough correctness: `default_input_config()` + `BufferSize::Default` is what makes the Scarlett and the NUX endpoint activate on the first try; the 250 ms rebuild is what stops the app dying when the Focusrite is power-cycled. |
| `src/model/notes.rs` | `Spelling` + `note_name` | Invariance boundary: `Spelling` may be read by nothing outside this file and the chip/label painters. |
| `src/ui/tuner_view.rs` | cent meter paint | The read-at-a-glance claim: dead band, tick density, edge-pin + chevron, monospace digits. |

## Verification

Working directory `D:\tuna` for every command. Fixtures go in `fixtures/` (create it).

1. **Model** — `cargo test --test model`. Asserts, with exact values: `midi_to_freq(29, 440.0) ∈ 43.65 ± 0.01`; `midi_to_freq(30, 440.0) ∈ 46.25 ± 0.01`; `note_name(29, Flat) == "F1"` and `note_name(29, Sharp) == "F1"` (MIDI 29 is a natural — the spelling tables must agree); `note_name(30, Sharp) == "F♯1"` and `note_name(30, Flat) == "G♭1"` (the one semitone, two spellings); `note_name(34, Flat) == "B♭1"`; `note_name(23, Sharp) == "B0"`; `note_name(60, Sharp) == "C4"`; `cents_between(41.203 * 2f32.powf(3.0/1200.0), 41.203) ≈ 3.0 ± 0.05`; `PRESETS` contains `guitar_8_f` = `[29,34,39,44,49,54,58,63]` and `guitar_8_fs` = `[30,35,40,45,50,55,59,64]`; every entry's MIDI ∈ 21..=108 and its lowest frequency ≥ `PITCH_FLOOR_HZ`; and for both spellings of the same MIDI, `cents_between` and `midi_to_freq` results are bit-identical (spelling cannot move the numbers).
2. **DSP fixtures** — `cargo test --release` (release: the fixtures are seconds of audio at 93.8 analyses/s). Each builds samples with the shared generator in `src/synth.rs` (same code `wavgen` uses, so a failing test can be reproduced as a WAV) and feeds `Analyzer::process_hop` in 512-sample hops — the same chain the live path uses:
   + `b0_locked`: `midi 23` (30.87 Hz), 8 harmonics `1/h`, `--noise -60`, onset 300 ms, 6 s → `Locked` by `t_ms < 700`; every `Locked` row has `target_midi == 23` and `|cents| ≤ 3`; in the last 1 s (settled), `max(f_hz) − min(f_hz)` corresponds to **< 5 ¢ peak-to-peak** (target < 3 ¢ — if it exceeds 5, apply the jitter contingency below, never loosen the assert).
   + `f1_locked`: `midi 29` (43.65 Hz, 8-string F standard low string) → `target_midi == 29`, `|cents| ≤ 3`, `f_hz ∈ 43.57..=43.73` (that interval is ±3 ¢ around 43.65 Hz).
   + `latency_vs_frequency`: identical file with `midi 23` vs `midi 40` (82.41 Hz) → first `Locked` row at `t_ms − onset < 400 ms` for MIDI 23 and `< 250 ms` for MIDI 40 (the `N = 4096` tracking window above 55 Hz is what earns the difference).
   + `weak_fundamental_octave`: `--fundamental-scale 0.25` on MIDI 23 → assert `state == Locked` implies `|1200·log2(f_hz / 30.87)| < 50 ¢` **or** `clarity < CLARITY_ACCEPT`; a confident octave error (≈ −1200 ¢ with high clarity) fails the test.
   + `note_plus_hum`: MIDI 23 at −12 dBFS plus `--hum -40` → `|cents| ≤ 3` and `target_midi == 23` throughout the settled region.
   + `silence_gates`: `--noise -85`, no tone → zero `Locked` rows, all rows `NoSignal`/`Idle`, `gate_db > rms_db`.
   + `drift_tracks`: `--drift 20` on MIDI 29 → settled `cents` span ≥ 25 ¢ peak-to-peak, proving the meter follows a real detune rather than snapping to 0 (the anti-"frozen needle" counter-check to the floating guards).
3. **Device enumeration** — `cargo run --release -- --list-devices`. Expect a row per capture endpoint including a `Focusrite USB Audio` entry whose id starts `wasapi:` and whose config line shows the mix format (expected `48000 Hz`, 2 ch, `F32`); if the NUX is plugged, its `USB Audio Device` row shows its own rate/channels. Any device row that fails to report a config must print the cpal error text, not blank.
4. **Offline end-to-end** — generate fixtures, then run them:
   `cargo run --release --bin wavgen -- --out fixtures/b0.wav --midi 23 --seconds 6 --onset 300 --harmonics 8 --noise -60 --seed 7`
   `cargo run --release --bin wavgen -- --out fixtures/f1.wav --midi 29 --seconds 6 --onset 300 --harmonics 8 --noise -60 --seed 11`
   then `cargo run --release -- --wav fixtures/b0.wav --preset bass_5 --csv`. Expected CSV: rows up to ~300 ms `NoSignal`, then a few `Acquiring`, then `Locked` with `target B0`, `f_hz ≈ 30.87`, `|cents| ≤ 3`, and hop-to-hop `cents` movement < 1 ¢ once settled. `--wav fixtures/f1.wav --preset guitar_8_f --csv` → `target F1`, `f_hz ≈ 43.65`.
5. **Visual check of the display** (needs no guitar): `cargo run --release --features eframe/__screenshot -- --wav fixtures/f1.wav --preset guitar_8_f` with `EFRAME_SCREENSHOT_TO` set to `shots/f1.png` (same shell: PowerShell `$env:EFRAME_SCREENSHOT_TO="shots/f1.png"; cargo run …`). The eframe `__screenshot` feature writes that PNG and exits. Inspect the image and confirm: big note reads `F1`, `target F1 · 43.65 Hz`, `43.65 Hz` and `≈ 0 ¢` in the readout row, the needle inside the green dead band, and the string chips exactly `F1 B♭1 E♭2 A♭2 D♭3 G♭3 B♭3 E♭4` — flats came from the preset suggestion, not from a default, which proves the preset→spelling rule and the ♯/♭ glyph fallback in one shot. Then re-run with `--spelling sharp`: chips must become exactly `F1 A♯1 D♯2 G♯2 C♯3 F♯3 A♯3 D♯4` and the big note still `F1`, while the Hz/¢ digits stay byte-identical to the flat run.
6. **Live capture, the real acceptance test** — `cargo run --release`. In the device combo pick `Focusrite USB Audio`; the format label must show `48 000 Hz · 2 ch · f32` and the level meter must move when you pluck. Pluck the low string of a bass or a down-tuned 8-string: state must go `Acquiring → Locked` in well under half a second, the note must be the correct octave (not an octave up), and with the string open and unmuted the needle must sit still: peak-to-peak movement **< 3 ¢** over 5 s with `Normal` smoothing. Click a string chip to pin, then strum a different string: the needle must not sweep across the dial — it pins at the edge with a chevron until you release `Auto`. Toggle ♯/♭ while locked: the Hz and ¢ digits must not change by a single character.
7. **Optional loopback proof** (only if Voicemeeter is running): play `fixtures/b0.wav` with the default Windows player into `Voicemeeter Input`, select `Voicemeeter VAIO` in Tuna → expect `B0` locked, same tolerances as step 6.
8. **Persistence** — after step 6, `cat "$APPDATA/tuna/config.toml"` must show the picked `device_id`, `preset = "bass_5"` (or whichever was chosen) and `spelling = "flat"` if toggled. Restart `cargo run --release`: device, preset and spelling come back; selecting `guitar_8_f` afterwards flips spelling to flat again (rule (d)).
9. **Ship it** — `pwsh ./build.ps1`, then run `dist\tuna.exe --list-devices` to prove the standalone exe needs nothing but itself.

## Assumptions & contingencies

+ **Search floor 27.5 Hz (A0)** is the deliberate boundary: it covers B0 30.87 Hz (5/6-string bass) and F1 43.65 Hz (8-string F standard) with ≥ 3 semitones of margin, and it keeps `τ_max = 1745` samples at 48 kHz so `W = 4096` still gives a 2.3-period correlation baseline. If a user needs anything below A0, `PITCH_FLOOR_HZ` is one constant — but then raise `W` to 5120 and re-check `W + τ_max ≤ 8192` before lowering it, or the low end starts returning octave errors. Re-run the `b0_locked`/`f1_locked` tests after any change.
+ **If ♯/♭ render as boxes**: `system_fonts` is enabled, so this is unexpected; if it happens, replace the two accidentals in `notes.rs`'s name tables and the toggle labels in `panels.rs` with `#`/`b`. Nothing else changes.
+ **If settled jitter exceeds 5 ¢ peak-to-peak** on the `b0_locked` fixture: widen the median to 5 samples and raise default `τ_smooth` to 200 ms. Do not relax the assertion — the display requirement is a still needle.
+ **If the `weak_fundamental_octave` fixture produces a confident octave error**: raise `YIN_THRESHOLD` to 0.15 and require 2 extra confirmation frames. If it is still wrong, the required outcome is honest failure display — the reading must land in `Unstable` with `clarity < CLARITY_ACCEPT` (that is what the test asserts), not a silently wrong note.
+ **If a Focusrite endpoint shows up twice** (a `… Wave` and a `… Streaming` row, which some driver installs do), prefer the row whose name does not contain `Wave`.
+ **If `default_input_config()` fails on the NUX device**, the fallback in step 4 (system default input + a visible diagnostics line carrying the cpal error) is the specified behaviour; do not invent a format.
+ **If first lock below 55 Hz feels sluggish in live use**: the honest floor is ~170 ms (3 periods of 30 Hz); the mitigation is `Pinned` string mode and `Fast` smoothing, not a shorter frame. Only consider `N = 6144 / W = 3072` if the `latency_vs_frequency` assert still passes at < 400 ms.
+ **egui 0.37 API drift** (`egui::Frame::new().corner_radius`, `SidePanel::exact_width`, `App::ui` vs `update`) shows up as plain compile errors; map each to the 0.37 docs page rather than downgrading egui.
+ Config uses `#[serde(default)]` only — an older file with unknown keys must still load, so do **not** add `deny_unknown_fields`.
