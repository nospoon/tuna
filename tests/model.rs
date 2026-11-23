//! Model-level guarantees: the 8-string presets, the note arithmetic that puts
//! B0 and F1 inside the search range, and the rule that spelling is display-only.

use tuna::model::config::AppConfig;
use tuna::model::notes::{
    cents_between, freq_to_midi, midi_to_freq, nearest_midi, note_name, note_parts, Spelling,
    PITCH_FLOOR_HZ,
};
use tuna::model::presets::{preset_by_id, tuning_for};

#[test]
fn eight_string_presets_are_one_semitone_apart_everywhere() {
    let sharp = preset_by_id("guitar_8_fs").unwrap();
    let flat = preset_by_id("guitar_8_f").unwrap();
    let a = tuning_for("guitar_8_fs", &[]);
    let b = tuning_for("guitar_8_f", &[]);
    assert_eq!(a.len(), 8);
    assert_eq!(b.len(), 8);
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(
            *x as i32 - *y as i32,
            1,
            "F♯ standard must sit exactly one semitone above F standard"
        );
        let df = cents_between(midi_to_freq(*x, 440.0), midi_to_freq(*y, 440.0));
        assert!((df - 100.0).abs() < 1e-3, "{x} vs {y} = {df} cents");
    }
    assert_eq!(sharp.spelling, Spelling::Sharp);
    assert_eq!(flat.spelling, Spelling::Flat);
    // 8-string F standard low string = 43.65 Hz, well above the search floor.
    assert!(midi_to_freq(29, 440.0) > PITCH_FLOOR_HZ);
}

#[test]
fn low_targets_land_on_the_expected_notes() {
    assert!((midi_to_freq(23, 440.0) - 30.87).abs() < 0.01);
    assert!((midi_to_freq(29, 440.0) - 43.65).abs() < 0.01);
    assert_eq!(note_name(23, Spelling::Sharp), "B0");
    assert_eq!(note_name(23, Spelling::Flat), "B0");
    assert_eq!(note_name(29, Spelling::Sharp), "F1");
    assert_eq!(note_name(29, Spelling::Flat), "F1");
    assert_eq!(note_name(30, Spelling::Sharp), "F♯1");
    assert_eq!(note_name(30, Spelling::Flat), "G♭1");
    assert_eq!(
        note_parts(30, Spelling::Flat),
        ("G♭".to_string(), "1".to_string())
    );
    assert_eq!(nearest_midi(30.87, 440.0), 23);
    assert_eq!(nearest_midi(43.65, 440.0), 29);
    // 87.31 Hz is F2, the exact double of F1: the name table names the note it
    // is handed. Staying on F1 rather than on its second harmonic is the pitch
    // stage's job (dsp::pitch, covered by tests/dsp.rs), not this one's.
    assert_eq!(nearest_midi(87.31, 440.0), 41);
    // Both printed labels are rounded: F1 is 43.6535 Hz (0.7 ¢ above 43.65) and
    // B0 is 30.8677 Hz (0.02 ¢ below 30.87), so the inverse map lands about 1.3
    // thousandths of a semitone off the exact note. A hundredth of a semitone
    // (1 ¢) is the tightest bound the labels themselves support.
    assert!((freq_to_midi(43.65, 440.0) - 29.0).abs() < 0.01);
    assert!((freq_to_midi(30.87, 440.0) - 23.0).abs() < 0.01);
}

#[test]
fn spelling_never_touches_a_single_number() {
    let freqs = [
        30.87f32, 30.9, 41.2, 43.65, 43.7, 49.0, 55.0, 82.41, 98.0, 123.47, 196.0, 329.63, 440.0,
    ];
    for f in freqs {
        for a4 in [435.0f32, 440.0, 455.0] {
            for spelling in [Spelling::Sharp, Spelling::Flat] {
                let cfg = AppConfig {
                    spelling,
                    a4_hz: a4,
                    tolerance_cents: 3.0,
                    ..Default::default()
                };
                assert_eq!(
                    numbers_at(f, &cfg),
                    numbers_at(
                        f,
                        &AppConfig {
                            spelling: other(spelling),
                            a4_hz: a4,
                            tolerance_cents: 3.0,
                            ..Default::default()
                        }
                    ),
                    "spelling changed a number at {f} Hz, a4 {a4}"
                );
            }
        }
    }
    // only the label may differ
    assert_eq!(
        note_name(33, Spelling::Sharp),
        note_name(33, Spelling::Flat)
    );
    assert_ne!(
        note_name(34, Spelling::Sharp),
        note_name(34, Spelling::Flat)
    );
    assert_eq!(
        note_parts(34, Spelling::Sharp).1,
        note_parts(34, Spelling::Flat).1
    );
}

fn other(s: Spelling) -> Spelling {
    match s {
        Spelling::Sharp => Spelling::Flat,
        Spelling::Flat => Spelling::Sharp,
    }
}

/// Every number the window prints for one measurement, as raw bits plus the
/// exact formatted digits a label would show.
fn numbers_at(f: f32, cfg: &AppConfig) -> (u8, u32, u32, u32, String) {
    let m = nearest_midi(f, cfg.a4_hz);
    let target = midi_to_freq(m, cfg.a4_hz);
    let cents = cents_between(f, target);
    (
        m,
        freq_to_midi(f, cfg.a4_hz).to_bits(),
        target.to_bits(),
        cents.to_bits(),
        format!(
            "{:.2} {:+.1} {:.1} {:.0}",
            f, cents, cfg.tolerance_cents, cfg.a4_hz
        ),
    )
}
