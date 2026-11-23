//! Tuning presets: MIDI numbers low string → high string, plus the accidental
//! spelling the tuning is normally written in.

use crate::model::notes::Spelling;

#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    /// MIDI numbers, low string first. Empty = chromatic (no target strings).
    pub midi: &'static [u8],
    /// Suggested spelling: flat-key tunings come up in flats.
    pub spelling: Spelling,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "chromatic",
        label: "Chromatic",
        midi: &[],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_standard",
        label: "Guitar standard E",
        midi: &[40, 45, 50, 55, 59, 64],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_drop_d",
        label: "Guitar drop D",
        midi: &[38, 45, 50, 55, 59, 64],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_dadgad",
        label: "DADGAD",
        midi: &[38, 45, 50, 55, 57, 62],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_open_g",
        label: "Open G",
        midi: &[38, 43, 50, 55, 59, 62],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_half_step",
        label: "Half step down (E\u{266D})",
        midi: &[39, 44, 49, 54, 58, 63],
        spelling: Spelling::Flat,
    },
    Preset {
        id: "guitar_drop_c",
        label: "Drop C",
        midi: &[36, 43, 48, 53, 57, 60],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_7_low_b",
        label: "7-string low B",
        midi: &[35, 40, 45, 50, 55, 59, 64],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_8_fs",
        label: "8-string F\u{266F} standard",
        midi: &[30, 35, 40, 45, 50, 55, 59, 64],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "guitar_8_f",
        label: "8-string F standard",
        midi: &[29, 34, 39, 44, 49, 54, 58, 63],
        spelling: Spelling::Flat,
    },
    Preset {
        id: "bass_4",
        label: "Bass 4-string E",
        midi: &[28, 33, 38, 43],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "bass_4_drop_d",
        label: "Bass drop D",
        midi: &[26, 33, 38, 43],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "bass_5",
        label: "Bass 5-string B",
        midi: &[23, 28, 33, 38, 43],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "bass_6",
        label: "Bass 6-string B",
        midi: &[23, 28, 33, 38, 43, 48],
        spelling: Spelling::Sharp,
    },
    Preset {
        id: "custom",
        label: "Custom",
        midi: &[],
        spelling: Spelling::Sharp,
    },
];

pub fn preset_by_id(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The tuning actually in force: preset table, or the user's list for
/// `custom`; `chromatic` has no strings.
pub fn tuning_for(id: &str, custom: &[u8]) -> Vec<u8> {
    match preset_by_id(id) {
        Some(p) if p.id == "custom" => custom.to_vec(),
        Some(p) => p.midi.to_vec(),
        None => custom.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_string_presets_are_adjacent_semitones_apart() {
        let fs = preset_by_id("guitar_8_fs").unwrap();
        let f = preset_by_id("guitar_8_f").unwrap();
        assert_eq!(fs.midi.len(), 8);
        assert_eq!(f.midi.len(), 8);
        for (a, b) in fs.midi.iter().zip(f.midi.iter()) {
            assert_eq!(*a as i16 - *b as i16, 1);
        }
        assert_eq!(f.spelling, Spelling::Flat);
        assert_eq!(fs.spelling, Spelling::Sharp);
    }
}
