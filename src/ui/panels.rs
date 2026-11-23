//! Side and top furniture: the I/O controls, the level meter, the harmonic
//! strip, the diagnostics log and the custom-tuning editor.
//!
//! The sharp/flat pair is a pair of 40 px buttons, not a settings entry: it is
//! the first thing a bassist reaches for when the same note wants to read A
//! sharp instead of B flat.

use egui::{Align2, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::audio::capture::AudioStatus;
use crate::audio::device::Inventory;
use crate::dsp::harmonics::NH;
use crate::dsp::track::{Hint, Reading};
use crate::model::config::{AppConfig, Smoothing, TOLERANCES};
use crate::model::notes::{
    midi_to_freq, note_name, Spelling, FLAT_GLYPH, MIDI_MAX, MIDI_MIN, SHARP_GLYPH,
};
use crate::model::presets::PRESETS;
use crate::ui::theme::{fade, signed, ACCENT, BG, BORDER, ERROR, FG, MUTED, SURFACE, WARNING};
use crate::ui::{spell, TargetMode};

/// What a control changed, so `TunaApp` knows whether to persist and whether to
/// rebuild the stream.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Changed {
    /// Stream parameters moved: the capture must be rebuilt.
    pub capture: bool,
    /// Something worth writing to `config.toml` moved.
    pub config: bool,
    /// The preset changed: its spelling suggestion applies.
    pub preset: bool,
}

impl Changed {
    pub fn merge(&mut self, other: Changed) {
        self.capture |= other.capture;
        self.config |= other.config;
        self.preset |= other.preset;
    }
}

pub struct Controls<'a> {
    pub cfg: &'a mut AppConfig,
    pub inventory: &'a Inventory,
    pub status: &'a AudioStatus,
    /// Live stream description, empty before the first successful open.
    pub format: &'a str,
    /// Newest-first analyzer/capture log lines.
    pub log: &'a [String],
    pub reading: Option<Reading>,
    /// `(gate_db, floor_db, open)` from the running analyzer.
    pub gate: (f32, f32, bool),
    /// Peak level held by the app, so the marker decays instead of flickering.
    pub peak_db: f32,
    /// Clip LED state, latched for a second by the app.
    pub clipped: bool,
    pub target: TargetMode,
}

const DB_FLOOR: f32 = -80.0;

// ------------------------------------------------------------- top bar -----

pub fn top_bar(ui: &mut Ui, c: &mut Controls<'_>) -> Changed {
    let mut ch = Changed::default();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ch.merge(controls_row(ui, c));
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let mut line = Changed::default();
        spelling_toggle(ui, c, &mut line);
        ui.add_space(14.0);
        tolerance_row(ui, c, &mut line);
        ui.add_space(14.0);
        smoothing_row(ui, c, &mut line);
        ch.merge(line);
    });

    if *c.status != AudioStatus::Running {
        let why = match c.status {
            AudioStatus::Failed(e) => e.clone(),
            AudioStatus::Idle => "capture stopped".to_string(),
            AudioStatus::Running => unreachable!(),
        };
        ui.label(RichText::new(why).size(13.0).color(ERROR));
    }
    ch
}

fn controls_row(ui: &mut Ui, c: &mut Controls<'_>) -> Changed {
    let mut ch = Changed::default();

    // Device: every enumerated input, including broken ones, so the user can
    // see why a device is unusable instead of wondering where it went.
    let selected = device_caption(c);
    let before = c.cfg.device_id.clone();
    egui::ComboBox::from_id_salt("device")
        .width(260.0)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for d in &c.inventory.devices {
                let mut label = d.name.clone();
                if d.is_default {
                    label.push_str("  (default)");
                }
                if let Some(e) = &d.error {
                    label.push_str(&format!("  [{e}]"));
                }
                ui.selectable_value(&mut c.cfg.device_id, d.id.clone(), label);
            }
        });
    if c.cfg.device_id != before {
        ch.capture = true;
        ch.config = true;
    }

    // Channel count comes from the device we are talking to, not from a guess.
    let channels = c
        .inventory
        .devices
        .iter()
        .find(|d| d.id == c.cfg.device_id)
        .map(|d| d.channels.max(1))
        .unwrap_or(1) as usize;
    let before_ch = c.cfg.input_channel;
    egui::ComboBox::from_id_salt("channel")
        .width(100.0)
        .selected_text(format!("Input {}", c.cfg.input_channel + 1))
        .show_ui(ui, |ui| {
            for i in 0..channels {
                ui.selectable_value(&mut c.cfg.input_channel, i, format!("Input {}", i + 1));
            }
        });
    if c.cfg.input_channel != before_ch {
        ch.capture = true;
        ch.config = true;
    }

    ui.label(
        RichText::new(grouped_hz(c.format))
            .monospace()
            .size(14.0)
            .color(MUTED),
    );

    ui.add_space(10.0);
    let before_a4 = c.cfg.a4_hz;
    ui.add(
        egui::DragValue::new(&mut c.cfg.a4_hz)
            .speed(0.5)
            .range(435.0..=455.0)
            .min_decimals(1)
            .max_decimals(1)
            .suffix(" Hz")
            .min_size(Vec2::new(96.0, 0.0)),
    );
    ui.label(RichText::new("A4").size(13.0).color(MUTED));
    if (c.cfg.a4_hz - before_a4).abs() > f32::EPSILON {
        ch.capture = true;
        ch.config = true;
    }

    let before_preset = c.cfg.preset.clone();
    let label = PRESETS
        .iter()
        .find(|p| p.id == c.cfg.preset)
        .map(|p| p.label)
        .unwrap_or("Custom");
    egui::ComboBox::from_id_salt("preset")
        .width(190.0)
        .selected_text(label)
        .show_ui(ui, |ui| {
            for p in PRESETS {
                ui.selectable_value(&mut c.cfg.preset, p.id.to_string(), p.label);
            }
        });
    if c.cfg.preset != before_preset {
        ch.capture = true;
        ch.preset = true;
        ch.config = true;
    }
    ch
}

fn device_caption(c: &Controls<'_>) -> String {
    match c.inventory.devices.iter().find(|d| d.id == c.cfg.device_id) {
        Some(d) if d.error.is_some() => format!("{} \u{2014} unusable", d.name),
        Some(d) => d.name.clone(),
        None if c.cfg.device_id.is_empty() => "System default".to_string(),
        None => "Device gone".to_string(),
    }
}

/// The plan's `48 000 Hz` grouping: a thin space inside the leading digit run.
fn grouped_hz(s: &str) -> String {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return s.to_string();
    }
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    out.push_str(&s[digits.len()..]);
    out
}

/// Two big adjacent buttons: the live spelling is filled accent.
fn spelling_toggle(ui: &mut Ui, c: &mut Controls<'_>, ch: &mut Changed) {
    for (glyph, value) in [(SHARP_GLYPH, Spelling::Sharp), (FLAT_GLYPH, Spelling::Flat)] {
        let active = c.cfg.spelling == value;
        let btn = egui::Button::new(
            RichText::new(spell(ui.ctx(), glyph))
                .size(26.0)
                .color(if active { BG } else { MUTED }),
        )
        .min_size(Vec2::new(52.0, 40.0))
        .corner_radius(8.0)
        .fill(if active { ACCENT } else { SURFACE })
        .stroke(Stroke::new(1.0, if active { ACCENT } else { BORDER }));
        if ui
            .add(btn)
            .on_hover_text("Note spelling: sharps or flats")
            .clicked()
        {
            c.cfg.spelling = value;
            ch.config = true;
        }
    }
}

fn tolerance_row(ui: &mut Ui, c: &mut Controls<'_>, ch: &mut Changed) {
    ui.label(RichText::new("in tune").size(13.0).color(MUTED));
    let before = c.cfg.tolerance_cents;
    for t in TOLERANCES {
        let label = format!("\u{00b1}{} \u{00a2}", t as i32);
        if ui
            .add(egui::Button::selectable(
                (c.cfg.tolerance_cents - t).abs() < f32::EPSILON,
                label,
            ))
            .clicked()
        {
            c.cfg.tolerance_cents = t;
        }
    }
    if (c.cfg.tolerance_cents - before).abs() > f32::EPSILON {
        ch.config = true;
    }
}

fn smoothing_row(ui: &mut Ui, c: &mut Controls<'_>, ch: &mut Changed) {
    ui.label(RichText::new("needle").size(13.0).color(MUTED));
    let before = c.cfg.smoothing;
    for s in Smoothing::ALL {
        if ui
            .add(egui::Button::selectable(c.cfg.smoothing == s, s.label()))
            .clicked()
        {
            c.cfg.smoothing = s;
        }
    }
    if c.cfg.smoothing != before {
        ch.capture = true;
        ch.config = true;
    }
}

// ----------------------------------------------------------- left panel ----

pub fn side_panel(ui: &mut Ui, c: &mut Controls<'_>) -> Changed {
    let mut ch = Changed::default();
    level_meter(ui, c);
    caption(ui, c);
    ui.add_space(14.0);
    ui.label(RichText::new("harmonics").size(13.0).color(MUTED));
    harmonic_strip(ui, c);
    ui.add_space(14.0);
    if c.cfg.preset == "custom" {
        ch.merge(tuning_editor(ui, c));
        ui.add_space(14.0);
    }
    ui.label(RichText::new("log").size(13.0).color(MUTED));
    ui.add_space(2.0);
    ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
        for line in c.log.iter().take(12) {
            ui.label(RichText::new(line).monospace().size(11.0).color(MUTED));
        }
    });
    ch
}

/// Level over −80..0 dBFS with 6 dB ticks, a held peak, the gate marker and a
/// latched clip LED: the interface gain is the single biggest cause of a
/// wandering needle, so its evidence lives on screen.
fn level_meter(ui: &mut Ui, c: &Controls<'_>) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w.max(1.0), 24.0), Sense::hover());
    let p = ui.painter();
    let x = |db: f32| {
        let t = (db - DB_FLOOR) / (0.0 - DB_FLOOR);
        rect.left() + t.clamp(0.0, 1.0) * rect.width()
    };

    p.rect_filled(rect, 3.0, SURFACE);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, BORDER), StrokeKind::Outside);
    for k in (0..=80).step_by(6) {
        let db = -k as f32;
        let h = if k % 24 == 0 { 8.0 } else { 4.0 };
        p.line(
            vec![
                Pos2::new(x(db), rect.bottom()),
                Pos2::new(x(db), rect.bottom() - h),
            ],
            Stroke::new(1.0, fade(MUTED, 0.4)),
        );
    }
    if let Some(r) = c.reading {
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left(), rect.top()),
                Pos2::new(x(r.rms_db), rect.bottom()),
            ),
            3.0,
            fade(ACCENT, 0.75),
        );
    }
    let pk = x(c.peak_db);
    p.line(
        vec![
            Pos2::new(pk, rect.top() + 1.0),
            Pos2::new(pk, rect.bottom() - 1.0),
        ],
        Stroke::new(2.0, FG),
    );
    let g = x(c.gate.0);
    p.line(
        vec![Pos2::new(g, rect.top()), Pos2::new(g, rect.bottom())],
        Stroke::new(1.5, MUTED),
    );

    let led = Rect::from_min_size(
        Pos2::new(rect.left(), rect.bottom() + 6.0),
        Vec2::new(12.0, 12.0),
    );
    p.rect_filled(led, 2.0, if c.clipped { ERROR } else { SURFACE });
    p.rect_stroke(led, 2.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    p.text(
        Pos2::new(led.right() + 6.0, led.center().y),
        Align2::LEFT_CENTER,
        "clip",
        FontId::proportional(11.0),
        MUTED,
    );
    ui.add_space(24.0);
}

fn caption(ui: &mut Ui, c: &Controls<'_>) {
    let (gate, floor, open) = c.gate;
    let text = format!(
        "gate {} dBFS \u{00b7} floor {} dBFS \u{00b7} {}",
        signed(gate, 1),
        signed(floor, 1),
        if open { "open" } else { "closed" }
    );
    ui.label(RichText::new(text).monospace().size(11.0).color(MUTED));
}

/// Twelve bars, h = 1..12, normalised to the strongest. The dashed line is the
/// ratio below which the analyzer calls the fundamental weak; the h=1 bar goes
/// amber under exactly that hint.
fn harmonic_strip(ui: &mut Ui, c: &Controls<'_>) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w.max(1.0), 68.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, SURFACE);

    let base = rect.bottom() - 13.0; // leave the label row
    let top = rect.top() + 4.0;
    let plot = base - top;

    let y0 = base - 0.25 * plot;
    let mut x = rect.left();
    while x < rect.right() {
        let nx = (x + 3.0).min(rect.right());
        p.line(
            vec![Pos2::new(x, y0), Pos2::new(nx, y0)],
            Stroke::new(1.0, fade(MUTED, 0.6)),
        );
        x = nx + 3.0;
    }

    let weak = matches!(
        c.reading.and_then(|r| r.hint),
        Some(Hint::WeakFundamental { .. })
    );
    let amps = c.reading.map(|r| r.harmonic_amps).unwrap_or([0.0; NH]);
    let slot = rect.width() / NH as f32;
    for (i, &a) in amps.iter().enumerate() {
        let bx = rect.left() + slot * (i as f32 + 0.15);
        let bw = slot * 0.7;
        let bh = a.clamp(0.0, 1.0) * plot;
        let colour = if i == 0 && weak { WARNING } else { ACCENT };
        p.rect_filled(
            Rect::from_min_max(Pos2::new(bx, base - bh), Pos2::new(bx + bw, base)),
            1.0,
            fade(colour, if a <= 0.0 { 0.25 } else { 0.85 }),
        );
        p.text(
            Pos2::new(bx + bw * 0.5, base + 2.0),
            Align2::CENTER_TOP,
            (i + 1).to_string(),
            FontId::proportional(10.0),
            MUTED,
        );
    }
}

/// One row per string, each stepping by one semitone inside 21..=108. Unisons
/// are legal, so nothing here forbids two rows on the same note.
fn tuning_editor(ui: &mut Ui, c: &mut Controls<'_>) -> Changed {
    let mut ch = Changed::default();
    ui.label(RichText::new("custom tuning").size(13.0).color(MUTED));
    let mut edits: Vec<(usize, i32)> = Vec::new();
    for (i, &m) in c.cfg.custom_tuning.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui
                .add(
                    egui::Button::new(RichText::new("\u{25c4}").size(13.0))
                        .min_size(Vec2::new(26.0, 22.0)),
                )
                .clicked()
            {
                edits.push((i, -1));
            }
            let label = format!(
                "{} {:.2} Hz",
                spell(ui.ctx(), &note_name(m, c.cfg.spelling)),
                midi_to_freq(m, c.cfg.a4_hz)
            );
            ui.label(RichText::new(label).size(13.0).color(FG));
            if ui
                .add(
                    egui::Button::new(RichText::new("\u{25ba}").size(13.0))
                        .min_size(Vec2::new(26.0, 22.0)),
                )
                .clicked()
            {
                edits.push((i, 1));
            }
        });
    }
    for (i, d) in edits {
        if i < c.cfg.custom_tuning.len() {
            let m = c.cfg.custom_tuning[i] as i32 + d;
            c.cfg.custom_tuning[i] = m.clamp(MIDI_MIN as i32, MIDI_MAX as i32) as u8;
            ch.capture = true;
            ch.config = true;
        }
    }
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(RichText::new("+ add string").size(13.0))
                    .min_size(Vec2::new(92.0, 22.0)),
            )
            .clicked()
        {
            // A new row starts at E4 (MIDI 64): mid-range and inside every
            // preset, so it is useful before it is stepped.
            c.cfg.custom_tuning.push(64);
            ch.capture = true;
            ch.config = true;
        }
        if ui
            .add(
                egui::Button::new(RichText::new("\u{2212} remove").size(13.0))
                    .min_size(Vec2::new(82.0, 22.0)),
            )
            .clicked()
            && !c.cfg.custom_tuning.is_empty()
        {
            c.cfg.custom_tuning.pop();
            ch.capture = true;
            ch.config = true;
        }
    });
    if ch.capture {
        // sorts ascending and clamps into range; unisons stay legal
        c.cfg.sanitize();
    }
    ch
}
