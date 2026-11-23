//! Note / frequency / cents model and the display-only accidental spelling.
//!
//! Invariant that the rest of the program relies on: nothing outside this module
//! (and the painters that call `note_name`) reads `Spelling`. Every `f32` in the
//! audio chain is a MIDI number or a hertz value, so flipping the spelling can
//! only ever relabel text — it cannot move a measurement.

/// Search floor: A0 = MIDI 21. Covers bass B0 (30.87 Hz) and 8-string F
/// standard F1 (43.65 Hz) with >= 3 semitones of margin, and keeps
/// `tau_max = fs / 27.5 = 1745` samples at 48 kHz so the 4096-sample
/// correlation baseline still spans ~2.3 periods at the floor.
pub const PITCH_FLOOR_HZ: f32 = 27.5;

/// Search ceiling: above the highest string of any preset (E5 = MIDI 76 is a
/// common high guitar string; 1300 Hz ≈ E6) so the harmonic strip and the
/// refinement window stay well below Nyquist.
pub const PITCH_CEIL_HZ: f32 = 1300.0;

pub const A4_DEFAULT: f32 = 440.0;

/// MIDI range accepted anywhere in the app (A0 .. C8).
pub const MIDI_MIN: u8 = 21;
pub const MIDI_MAX: u8 = 108;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Spelling {
    Sharp,
    Flat,
}

const SHARP_NAMES: [&str; 12] = [
    "C",
    "C\u{266F}",
    "D",
    "D\u{266F}",
    "E",
    "F",
    "F\u{266F}",
    "G",
    "G\u{266F}",
    "A",
    "A\u{266F}",
    "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C",
    "D\u{266D}",
    "D",
    "E\u{266D}",
    "E",
    "F",
    "G\u{266D}",
    "G",
    "A\u{266D}",
    "A",
    "B\u{266D}",
    "B",
];

/// The two accidentals, single place so a font without the glyphs is a
/// one-line change to "#" / "b".
pub const SHARP_GLYPH: &str = "\u{266F}";
pub const FLAT_GLYPH: &str = "\u{266D}";

pub fn midi_to_freq(m: u8, a4: f32) -> f32 {
    a4 * 2f32.powf((m as f32 - 69.0) / 12.0)
}

pub fn freq_to_midi(f: f32, a4: f32) -> f32 {
    69.0 + 12.0 * (f / a4).log2()
}

pub fn cents_between(f_meas: f32, f_target: f32) -> f32 {
    if f_meas <= 0.0 || f_target <= 0.0 || !f_meas.is_finite() || !f_target.is_finite() {
        return 0.0;
    }
    1200.0 * (f_meas / f_target).log2()
}

/// Scientific pitch notation: C4 = MIDI 60, so MIDI 23 = `B0`, MIDI 29 = `F1`.
pub fn note_name(m: u8, spelling: Spelling) -> String {
    let table = match spelling {
        Spelling::Sharp => &SHARP_NAMES,
        Spelling::Flat => &FLAT_NAMES,
    };
    let letter = table[(m % 12) as usize];
    let octave = (m as i32) / 12 - 1;
    format!("{letter}{octave}")
}

/// Split a note name into letter+accidental and octave, for the big display
/// glyph where the accidental is drawn at a different size.
pub fn note_parts(m: u8, spelling: Spelling) -> (String, String) {
    let name = note_name(m, spelling);
    match name.rfind(|c: char| c.is_ascii_digit() || c == '-') {
        Some(i) => (name[..i].to_string(), name[i..].to_string()),
        None => (name.clone(), String::new()),
    }
}

/// Nearest in-tune MIDI number for a measured frequency, clamped to the range
/// the app accepts.
pub fn nearest_midi(f: f32, a4: f32) -> u8 {
    let m = freq_to_midi(f, a4).round();
    let lo = freq_to_midi(PITCH_FLOOR_HZ, a4);
    let hi = freq_to_midi(PITCH_CEIL_HZ, a4);
    m.clamp(lo, hi).clamp(MIDI_MIN as f32, MIDI_MAX as f32) as u8
}

/// Target for a measured frequency: the nearest string of the selected tuning,
/// or the nearest semitone when the tuning is empty (chromatic mode). Preset
/// modes need the tuning rows as the candidate set — otherwise a string sitting
/// more than 50 ¢ flat highlights its neighbour's chip, and the `WrongPreset`
/// hint could never fire because a chromatic target is always within 50 ¢.
pub fn nearest_target(f: f32, a4: f32, tuning: &[u8]) -> u8 {
    if tuning.is_empty() {
        return nearest_midi(f, a4);
    }
    let m = freq_to_midi(f, a4);
    let pick = tuning
        .iter()
        .min_by(|a, b| {
            (f32::from(**a) - m)
                .abs()
                .partial_cmp(&(f32::from(**b) - m).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied();
    pick.unwrap_or_else(|| nearest_midi(f, a4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naturals_are_spelled_identically() {
        assert_eq!(
            note_name(29, Spelling::Sharp),
            note_name(29, Spelling::Flat)
        );
    }

    #[test]
    fn nearest_midi_clamps_to_the_accepted_range() {
        // the analyser's window is 27.5 Hz .. 1300 Hz: out-of-window input
        // clamps to the nearest note inside it, never to a note it cannot measure
        assert_eq!(nearest_midi(10.0, 440.0), MIDI_MIN);
        assert_eq!(nearest_midi(PITCH_FLOOR_HZ, 440.0), MIDI_MIN);
        assert_eq!(
            nearest_midi(9000.0, 440.0),
            nearest_midi(PITCH_CEIL_HZ, 440.0)
        );
        assert!(
            nearest_midi(PITCH_CEIL_HZ, 440.0) < MIDI_MAX,
            "ceiling must clamp before MIDI_MAX"
        );
        assert_eq!(nearest_midi(440.0, 440.0), 69);
        assert_eq!(nearest_midi(30.87, 440.0), 23);
    }

    #[test]
    fn a_selected_tuning_limits_which_note_can_be_targeted() {
        let tuning = [29u8, 34, 39];
        // 60 ¢ under F1: the nearest chromatic semitone is E1 (28), the nearest
        // string of this tuning is F1.
        let flat_of_f1 = midi_to_freq(29, A4_DEFAULT) * 2f32.powf(-60.0 / 1200.0);
        assert_eq!(nearest_midi(flat_of_f1, A4_DEFAULT), 28);
        assert_eq!(nearest_target(flat_of_f1, A4_DEFAULT, &tuning), 29);
        assert_eq!(
            nearest_target(flat_of_f1, A4_DEFAULT, &[]),
            nearest_midi(flat_of_f1, A4_DEFAULT)
        );
    }

    #[test]
    fn note_parts_splits_the_octave_off() {
        assert_eq!(
            note_parts(30, Spelling::Flat),
            ("G\u{266D}".into(), "1".into())
        );
    }
}
