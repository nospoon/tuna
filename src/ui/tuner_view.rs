//! Central panel: chips, note glyph, the plus/minus 50 cent meter, numeric
//! readout, hint line.
//!
//! Every value painted here arrives already computed: the meter reads
//! `Reading::cents`, the glyph reads `Reading::target_midi`. Spelling only ever
//! reaches the label painters, so flipping it moves no pixel of the meter and
//! changes no digit.

use egui::{
    Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::dsp::track::{Hint, Reading, TrackState};
use crate::model::notes::{midi_to_freq, note_name, note_parts, Spelling};
use crate::ui::theme::{
    fade, signed, ACCENT, BG, BORDER, ERROR, FG, MUTED, SUCCESS, SURFACE, WARNING,
};
use crate::ui::{spell, TargetMode};

/// Half-range of the meter, in cents.
pub const METER_RANGE: f32 = 50.0;
const METER_H: f32 = 128.0;
const GLYPH_H: f32 = 172.0;
const LETTER: f32 = 150.0;
const ACCIDENTAL: f32 = 96.0;
const OCTAVE: f32 = 72.0;
const MINUS: char = '\u{2212}';
const DASH: char = '\u{2014}';
const CENT: char = '\u{00a2}';
const DOT: char = '\u{00b7}';

/// Everything the central panel draws. Built fresh each frame by `TunaApp`.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub reading: Option<Reading>,
    pub tuning: &'a [u8],
    pub spelling: Spelling,
    pub a4: f32,
    pub target: TargetMode,
    pub tolerance: f32,
    pub in_tune: bool,
    /// Overrides the state chip when capture itself is the story
    /// (`Disconnected`, `Recovering`).
    pub status: Option<&'a str>,
}

impl<'a> View<'a> {
    fn state(&self) -> TrackState {
        self.reading.map(|r| r.state).unwrap_or(TrackState::Idle)
    }
    /// No note name before the tracker has a target: during acquisition there
    /// is no note to commit to, and inventing one is exactly the floating the
    /// app is built to avoid.
    fn note(&self) -> Option<u8> {
        self.reading.and_then(|r| r.target_midi)
    }
}

/// Paints the central panel. Returns the chip the user clicked this frame.
pub fn show(ui: &mut Ui, v: &View<'_>) -> Option<TargetMode> {
    let mut clicked = None;
    if !v.tuning.is_empty() {
        clicked = chips(ui, v);
    }
    note_glyph(ui, v);
    ui.add_space(4.0);
    cent_meter(ui, v);
    ui.add_space(6.0);
    readout(ui, v);
    hint_line(ui, v);
    clicked
}

// ---------------------------------------------------------------- chips ----

fn chips(ui: &mut Ui, v: &View<'_>) -> Option<TargetMode> {
    let mut clicked = None;
    let target = v.reading.and_then(|r| r.target_midi);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if chip(ui, "Auto", v.target == TargetMode::Auto, false) {
            clicked = Some(TargetMode::Auto);
        }
        for (i, &m) in v.tuning.iter().enumerate() {
            let active = v.target == TargetMode::Pinned(i);
            if chip(
                ui,
                &spell(ui.ctx(), &note_name(m, v.spelling)),
                active,
                target == Some(m),
            ) {
                clicked = Some(TargetMode::Pinned(i));
            }
        }
    });
    ui.add_space(8.0);
    clicked
}

/// Filled accent when pinned, accent-outlined when it is the current target.
fn chip(ui: &mut Ui, label: &str, active: bool, outline: bool) -> bool {
    let b = egui::Button::new(
        RichText::new(label)
            .size(15.0)
            .color(if active { BG } else { FG }),
    )
    .min_size(Vec2::new(46.0, 27.0))
    .corner_radius(6.0)
    .fill(if active { ACCENT } else { SURFACE })
    .stroke(Stroke::new(
        if outline { 1.5 } else { 1.0 },
        if outline { ACCENT } else { BORDER },
    ));
    ui.add(b).clicked()
}

// ------------------------------------------------------------- the glyph ---

fn note_glyph(ui: &mut Ui, v: &View<'_>) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w.max(1.0), GLYPH_H), Sense::hover());
    let p = ui.painter();
    let Some(m) = v.note() else { return };

    let locked = v.state() == TrackState::Locked;
    let colour = if locked {
        if v.in_tune {
            SUCCESS
        } else {
            FG
        }
    } else {
        // held value: the analyzer froze the last good reading, say so quietly
        fade(MUTED, 0.4)
    };

    let (head, octave) = note_parts(m, v.spelling);
    let letter: String = head.chars().next().unwrap_or_default().to_string();
    let accidentals: String = spell(ui.ctx(), &head.chars().skip(1).collect::<String>());
    let baseline = Pos2::new(rect.left() + 8.0, rect.bottom() - 14.0);

    let mut x = baseline.x;
    for (text, size) in [
        (letter.as_str(), LETTER),
        (accidentals.as_str(), ACCIDENTAL),
        (octave.as_str(), OCTAVE),
    ] {
        if text.is_empty() {
            continue;
        }
        // egui 0.37 dropped `Align2::LEFT_BASELINE`. Lay the piece out, then
        // drop it so that the ink bottom of a `0` in the same font — a digit
        // sits exactly on the baseline — lands on the shared baseline.
        let font = FontId::proportional(size);
        let galley = p.layout_no_wrap(text.to_string(), font.clone(), colour);
        let probe = p.layout_no_wrap("0".to_string(), font, colour);
        let at = Pos2::new(x, baseline.y - probe.mesh_bounds.bottom());
        p.galley(at, galley.clone(), colour);
        x += galley.rect.width();
    }

    // Target and its theoretical pitch, right of the glyph.
    p.text(
        Pos2::new(x + 26.0, rect.bottom() - 48.0),
        Align2::LEFT_TOP,
        format!(
            "target {} {} {:.2} Hz",
            spell(ui.ctx(), &note_name(m, v.spelling)),
            DOT,
            midi_to_freq(m, v.a4)
        ),
        FontId::proportional(18.0),
        MUTED,
    );
}

// --------------------------------------------------------------- the meter -

fn cent_meter(ui: &mut Ui, v: &View<'_>) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w.max(1.0), METER_H), Sense::hover());
    let p = ui.painter();

    // the word labels own the ends; the cent scale lives between them
    let track = Rect::from_min_max(
        Pos2::new(rect.left() + 54.0, rect.top() + 6.0),
        Pos2::new(rect.right() - 54.0, rect.bottom() - 26.0),
    );
    let centre = (track.left() + track.right()) * 0.5;
    let half = ((track.right() - track.left()) * 0.5).max(1.0);
    let x = |c: f32| centre + (c.clamp(-METER_RANGE, METER_RANGE) / METER_RANGE) * half;

    p.rect_filled(track, 4.0, SURFACE);
    p.rect_stroke(track, 4.0, Stroke::new(1.0, BORDER), StrokeKind::Outside);

    let tol = v.tolerance.clamp(0.5, METER_RANGE);
    p.rect_filled(
        Rect::from_min_max(
            Pos2::new(x(-tol), track.top()),
            Pos2::new(x(tol), track.bottom()),
        ),
        3.0,
        fade(SUCCESS, 0.12),
    );

    let bottom = track.bottom();
    for c in (-50..=50).step_by(5) {
        let major = c % 10 == 0;
        let h = if major { 13.0 } else { 6.0 };
        p.line(
            vec![
                Pos2::new(x(c as f32), bottom),
                Pos2::new(x(c as f32), bottom - h),
            ],
            Stroke::new(1.0, fade(MUTED, if major { 0.9 } else { 0.45 })),
        );
    }
    for (c, label) in [
        (-50.0, format!("{MINUS}50")),
        (-25.0, format!("{MINUS}25")),
        (0.0, "0".to_string()),
        (25.0, "+25".to_string()),
        (50.0, format!("+50 {CENT}")),
    ] {
        p.text(
            Pos2::new(x(c), bottom + 5.0),
            Align2::CENTER_TOP,
            label,
            FontId::proportional(13.0),
            MUTED,
        );
    }

    // centre line: the target, 2 px so it never vanishes under the needle
    p.line(
        vec![
            Pos2::new(centre, track.top() + 3.0),
            Pos2::new(centre, bottom - 3.0),
        ],
        Stroke::new(2.0, FG),
    );

    p.text(
        Pos2::new(rect.left() + 8.0, track.center().y),
        Align2::LEFT_CENTER,
        "FLAT",
        FontId::proportional(13.0),
        MUTED,
    );
    p.text(
        Pos2::new(rect.right() - 8.0, track.center().y),
        Align2::RIGHT_CENTER,
        "SHARP",
        FontId::proportional(13.0),
        MUTED,
    );

    let Some(r) = v.reading else { return };
    if r.state == TrackState::Idle || r.state == TrackState::NoSignal {
        return;
    }
    let colour = if v.in_tune {
        SUCCESS
    } else if r.state == TrackState::Locked {
        WARNING
    } else {
        MUTED
    };
    let top = track.top() + track.height() * 0.1; // 0.8 of the panel height
    let nx = x(r.cents);
    let needle = Rect::from_min_max(Pos2::new(nx - 2.5, top), Pos2::new(nx + 2.5, bottom - 14.0));
    p.rect_filled(needle, 2.5, colour);
    // one pixel of the background around it, so it reads over the dead band
    p.rect_stroke(
        needle.expand(1.0),
        3.5,
        Stroke::new(1.0, BG),
        StrokeKind::Outside,
    );

    // Past the end of the scale the needle pins to the rail and a chevron says
    // further out; the numeric readout keeps telling the truth.
    if r.cents.abs() > METER_RANGE {
        let dir = r.cents.signum();
        let tip = if dir < 0.0 {
            track.left() - 8.0
        } else {
            track.right() + 8.0
        };
        let mid = track.center().y;
        p.add(Shape::convex_polygon(
            vec![
                Pos2::new(tip, mid),
                Pos2::new(tip - 9.0 * dir, mid - 10.0),
                Pos2::new(tip - 9.0 * dir, mid + 10.0),
            ],
            colour,
            Stroke::NONE,
        ));
    }
}

// -------------------------------------------------------------- readout ----

fn readout(ui: &mut Ui, v: &View<'_>) {
    let r = v.reading;
    let live = matches!(
        v.state(),
        TrackState::Locked | TrackState::Acquiring | TrackState::Unstable
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let hz = if live {
            format!("{:>7.2} Hz", r.map(|r| r.f_hz).unwrap_or(0.0))
        } else {
            format!("   {MINUS}{MINUS}{MINUS}.{MINUS} Hz")
        };
        ui.label(RichText::new(hz).monospace().size(30.0).color(if live {
            FG
        } else {
            fade(MUTED, 0.5)
        }));

        let cents = if live {
            format!(
                "{} {CENT}",
                signed(r.map(|r| r.cents).unwrap_or(0.0).clamp(-1200.0, 1200.0), 1)
            )
        } else {
            format!("  {MINUS}{MINUS}.{MINUS} {CENT}")
        };
        let ccol = if !live {
            fade(MUTED, 0.5)
        } else if v.in_tune {
            SUCCESS
        } else {
            FG
        };
        ui.label(RichText::new(cents).monospace().size(30.0).color(ccol));

        let (text, colour) = state_chip(v);
        let _ = egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(6.0)
            .stroke(Stroke::new(1.0, BORDER))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.label(RichText::new(text).size(15.0).color(colour));
            });
    });
}

fn state_chip(v: &View<'_>) -> (String, Color32) {
    if let Some(s) = v.status {
        return (s.to_string(), ERROR);
    }
    match v.state() {
        TrackState::Idle => ("Idle".into(), MUTED),
        TrackState::NoSignal => ("No signal".into(), MUTED),
        TrackState::Acquiring => ("Acquiring".into(), MUTED),
        TrackState::Unstable => ("Unstable".into(), WARNING),
        // The window the analyzer actually ran this hop: `N_TRACK`, not
        // `N_MAX`, whenever it is locked above `F_TRACK_SHORT_HZ`.
        TrackState::Locked => match v.reading {
            Some(r) => (format!("Locked {} {} ms", DOT, r.window_ms), SUCCESS),
            None => ("Locked".into(), SUCCESS),
        },
    }
}

fn hint_line(ui: &mut Ui, v: &View<'_>) {
    let Some(r) = v.reading else { return };
    let Some(h) = r.hint else { return };
    let text = match h {
        Hint::LowSignal => format!(
            "Signal below gate ({} dBFS) {DASH} raise interface gain",
            signed(r.rms_db, 1)
        ),
        Hint::Clipping => format!("Clipping {DASH} lower interface gain"),
        Hint::WeakFundamental { strongest_h } => format!(
            "Fundamental weak: strongest harmonic h={strongest_h} {DASH} check input/impedance"
        ),
        Hint::WrongPreset { semitones } => {
            let note = r
                .target_midi
                .map(|m| spell(ui.ctx(), &note_name(m, v.spelling)))
                .unwrap_or_else(|| "the selected string".to_string());
            let side = if r.cents < 0.0 { "below" } else { "above" };
            format!(
                "Measured pitch is {semitones} semitones {side} {note} {DASH} wrong tuning selected?"
            )
        }
    };
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(16.0).color(MUTED));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::harmonics::NH;

    fn view(reading: Option<Reading>) -> View<'static> {
        View {
            reading,
            tuning: &[],
            spelling: Spelling::Sharp,
            a4: 440.0,
            target: TargetMode::Auto,
            tolerance: 3.0,
            in_tune: false,
            status: None,
        }
    }

    /// The state chip is the only place the analysis window is visible, so it
    /// has to come from the reading in front of it: locked at 110 Hz the
    /// analyzer correlated 4 096 samples (85 ms), locked on F1 it correlated
    /// 8 192 (171 ms). A constant here advertised the long frame for every
    /// note above A1.
    #[test]
    fn the_locked_chip_quotes_the_window_of_its_reading() {
        let locked = Reading {
            t_ms: 1_000,
            window_ms: 85,
            f_hz: 110.0,
            cents: 0.0,
            clarity: 0.9,
            state: TrackState::Locked,
            target_midi: Some(45),
            rms_db: -20.0,
            peak_db: -14.0,
            gate_db: -60.0,
            clipped: false,
            harmonic_amps: [0.0; NH],
            hint: None,
        };
        assert_eq!(
            state_chip(&view(Some(locked))).0,
            format!("Locked {DOT} 85 ms")
        );
        let bass = Reading {
            window_ms: 171,
            ..locked
        };
        assert_eq!(
            state_chip(&view(Some(bass))).0,
            format!("Locked {DOT} 171 ms")
        );
    }
}
