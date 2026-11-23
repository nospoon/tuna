//! The palette. egui's own `Visuals::dark()` is left completely alone (its
//! fields churn between releases); every colour Tuna paints comes from here,
//! applied explicitly to frames, rich text and custom shapes.

use egui::Color32;

pub const BG: Color32 = Color32::from_rgb(0x10, 0x10, 0x14);
pub const SURFACE: Color32 = Color32::from_rgb(0x17, 0x17, 0x1D);
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x33);
pub const FG: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xEC);
pub const MUTED: Color32 = Color32::from_rgb(0x8C, 0x8C, 0x99);
pub const ACCENT: Color32 = Color32::from_rgb(0x4C, 0xC2, 0xFF);
pub const SUCCESS: Color32 = Color32::from_rgb(0x3F, 0xD3, 0x7A);
pub const WARNING: Color32 = Color32::from_rgb(0xF0, 0xA6, 0x3C);
pub const ERROR: Color32 = Color32::from_rgb(0xE8, 0x54, 0x5E);

/// `c` at `alpha` opacity over whatever is behind it.
pub fn fade(c: Color32, alpha: f32) -> Color32 {
    c.linear_multiply(alpha)
}

/// `−4.3`, `+0.0`: U+2212 rather than a hyphen, so the cent readout matches the
/// plan's typography and never reads as a bullet.
pub fn signed(v: f32, dp: usize) -> String {
    let digits = format!("{:.*}", dp, v.abs());
    if v.is_sign_negative() {
        format!("\u{2212}{digits}")
    } else {
        format!("+{digits}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_uses_a_real_minus_and_a_leading_plus() {
        assert_eq!(signed(-4.32, 1), "\u{2212}4.3");
        assert_eq!(signed(4.32, 1), "+4.3");
        assert_eq!(signed(0.0, 1), "+0.0");
        assert_eq!(signed(-0.04, 1), "\u{2212}0.0");
    }
}
