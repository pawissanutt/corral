//! WCAG AA for the theme's text tokens, computed from `theme.json` itself, so a retuned colour
//! that drops quiet text, a chip's words or the failing health pill under 4.5:1 fails here.

use serde_json::Value;

const AA: f64 = 4.5;

/// The grounds quiet text and line numbers sit on: pages, cards, wells, the diff tints and the
/// selected and hovered rows.
const GROUNDS: [&str; 8] = [
    "background",
    "surface",
    "surface-2",
    "bg-sunk",
    "ok-bg",
    "fail-bg",
    "sel",
    "hover",
];
const HUES: [&str; 6] = ["you", "corral", "worker", "held", "fail", "ok"];

fn theme() -> Value {
    serde_json::from_str(include_str!("../../theme.json")).unwrap_or(Value::Null)
}

/// `oklch(L C h)` as linear sRGB, clipped to the gamut as a browser draws it.
fn linear(value: &str) -> [f64; 3] {
    let inner = value
        .trim_start_matches("oklch(")
        .trim_end_matches(')')
        .split('/')
        .next()
        .unwrap_or_default();
    let parts: Vec<f64> = inner
        .split_whitespace()
        .map(|part| part.parse().unwrap_or(f64::NAN))
        .collect();
    let [l, c, h] = parts[..] else {
        panic!("not an oklch colour: {value}");
    };
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    [
        4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
        -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_,
    ]
    .map(|channel| channel.clamp(0.0, 1.0))
}

/// Relative luminance after the 8-bit sRGB round trip WCAG measures.
fn luminance(value: &str) -> f64 {
    let [r, g, b] = linear(value).map(|channel| {
        let encoded = if channel <= 0.003_130_8 {
            12.92 * channel
        } else {
            1.055 * channel.powf(1.0 / 2.4) - 0.055
        };
        let byte = (encoded * 255.0).round() / 255.0;
        if byte <= 0.040_45 {
            byte / 12.92
        } else {
            ((byte + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(ink: &str, ground: &str) -> f64 {
    let (a, b) = (luminance(ink), luminance(ground));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn token<'a>(theme: &'a Value, appearance: &str, key: &str) -> &'a str {
    theme["cssVars"][appearance][key]
        .as_str()
        .unwrap_or_else(|| panic!("theme.json lacks {appearance} {key}"))
}

#[test]
fn quiet_text_and_failing_health_pill_meet_aa_contrast() {
    let theme = theme();
    let mut failures = Vec::new();
    for appearance in ["light", "dark"] {
        let mut check = |ink: &str, ground: &str| {
            let ratio = contrast(
                token(&theme, appearance, ink),
                token(&theme, appearance, ground),
            );
            if ratio < AA {
                failures.push(format!("{appearance} {ink} on {ground}: {ratio:.2}"));
            }
        };
        for ink in ["ink-3", "ink-4"] {
            for ground in GROUNDS {
                check(ink, ground);
            }
        }
        for hue in HUES {
            check(hue, &format!("{hue}-bg"));
            check(hue, "background");
        }
    }
    // The failing health segment: the dark appearance's fail fill under the shell's own dark, in
    // both appearances, as the always-dark shell draws it.
    let pill = contrast(
        token(&theme, "dark", "shell"),
        token(&theme, "dark", "fail"),
    );
    if pill < AA {
        failures.push(format!("health pill shell on fail: {pill:.2}"));
    }
    assert!(failures.is_empty(), "under AA: {failures:#?}");
}
