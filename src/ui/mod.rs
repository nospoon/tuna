//! The drawing layer. It sees plain values (`View`, `Controls`) and reports
//! what the user clicked; it never touches cpal, the analyzer or the config
//! file. `TargetMode` lives here because it is view state, not persisted
//! settings — `AppConfig::pinned_string` is its serialised form.

pub mod panels;
pub mod theme;
pub mod tuner_view;

use std::sync::OnceLock;

use egui::{Context, FontId};

use crate::model::notes::{FLAT_GLYPH, SHARP_GLYPH};

/// Which string the analysis is restricted to. `Pinned(i)` indexes the selected
/// tuning and turns on the ±120 ¢ restricted search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TargetMode {
    #[default]
    Auto,
    Pinned(usize),
}

impl TargetMode {
    pub fn index(self) -> Option<usize> {
        match self {
            TargetMode::Auto => None,
            TargetMode::Pinned(i) => Some(i),
        }
    }
}

/// The `model` name tables always carry U+266F and U+266D. Neither is
/// guaranteed a face: egui's Windows fallback queries the generic sans,
/// system-ui and emoji families, and a set that carries `♯` but not `♭` paints
/// the flat as a notdef box. Ask the font engine once, and if either glyph is
/// missing spell both as ASCII, so the two spellings stay consistent. Layout
/// only: the letters, digits and every `f32` are untouched.
fn ascii_accidentals(ctx: &Context) -> bool {
    static ASCII: OnceLock<bool> = OnceLock::new();
    *ASCII.get_or_init(|| {
        ctx.fonts_mut(|fonts| {
            let id = FontId::proportional(26.0);
            !(fonts.has_glyph(&id, '\u{266F}') && fonts.has_glyph(&id, '\u{266D}'))
        })
    })
}

/// A note name as this machine's installed fonts can draw it.
pub fn spell(ctx: &Context, name: &str) -> String {
    substitute(ascii_accidentals(ctx), name)
}

fn substitute(ascii: bool, name: &str) -> String {
    if ascii {
        name.replace(SHARP_GLYPH, "#").replace(FLAT_GLYPH, "b")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_font_without_the_accidental_gets_the_ascii_spelling() {
        assert_eq!(substitute(true, "B\u{266D}1"), "Bb1");
        assert_eq!(substitute(true, "F\u{266F}3"), "F#3");
        assert_eq!(substitute(true, "F1"), "F1");
        assert_eq!(substitute(false, "B\u{266D}1"), "B\u{266D}1");
    }
}
