use anyhow::Result;
use serde::{Deserialize, Serialize};

// ============================================================================
// Theme Color Structures
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BaseColors {
    #[serde(rename = "950")]
    pub c950: String,
    #[serde(rename = "900")]
    pub c900: String,
    #[serde(rename = "800")]
    pub c800: String,
    #[serde(rename = "700")]
    pub c700: String,
    #[serde(rename = "600")]
    pub c600: String,
    #[serde(rename = "500")]
    pub c500: String,
    #[serde(rename = "400")]
    pub c400: String,
    #[serde(rename = "300")]
    pub c300: String,
    #[serde(rename = "200")]
    pub c200: String,
    #[serde(rename = "100")]
    pub c100: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AccentColors {
    #[serde(rename = "600")]
    pub c600: String,
    #[serde(rename = "500")]
    pub c500: String,
    #[serde(rename = "400")]
    pub c400: String,
    #[serde(rename = "300")]
    pub c300: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThemeConfig {
    pub base: BaseColors,
    pub primary: AccentColors,
    pub secondary: AccentColors,
    pub error: AccentColors,
    pub success: AccentColors,
    pub info: AccentColors,
    pub warning: AccentColors,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CustomThemeEntry {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub theme: ThemeConfig,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// Default Implementations
// ============================================================================

impl Default for BaseColors {
    fn default() -> Self {
        Self {
            c950: "#020617".to_string(),
            c900: "#0f172a".to_string(),
            c800: "#1e293b".to_string(),
            c700: "#334155".to_string(),
            c600: "#475569".to_string(),
            c500: "#64748b".to_string(),
            c400: "#94a3b8".to_string(),
            c300: "#cbd5e1".to_string(),
            c200: "#e2e8f0".to_string(),
            c100: "#f1f5f9".to_string(),
        }
    }
}

impl Default for AccentColors {
    fn default() -> Self {
        Self {
            c600: "#0d9488".to_string(),
            c500: "#14b8a6".to_string(),
            c400: "#2dd4bf".to_string(),
            c300: "#5eead4".to_string(),
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            base: BaseColors::default(),
            primary: AccentColors::default(),
            secondary: AccentColors {
                c600: "#9333ea".to_string(),
                c500: "#a855f7".to_string(),
                c400: "#c084fc".to_string(),
                c300: "#d8b4fe".to_string(),
            },
            error: AccentColors {
                c600: "#dc2626".to_string(),
                c500: "#ef4444".to_string(),
                c400: "#f87171".to_string(),
                c300: "#fca5a5".to_string(),
            },
            success: AccentColors {
                c600: "#16a34a".to_string(),
                c500: "#22c55e".to_string(),
                c400: "#4ade80".to_string(),
                c300: "#86efac".to_string(),
            },
            info: AccentColors {
                c600: "#2563eb".to_string(),
                c500: "#3b82f6".to_string(),
                c400: "#60a5fa".to_string(),
                c300: "#93c5fd".to_string(),
            },
            warning: AccentColors {
                c600: "#d97706".to_string(),
                c500: "#f59e0b".to_string(),
                c400: "#fbbf24".to_string(),
                c300: "#fcd34d".to_string(),
            },
        }
    }
}

fn semantic_error() -> AccentColors {
    AccentColors {
        c600: "#dc2626".to_string(),
        c500: "#ef4444".to_string(),
        c400: "#f87171".to_string(),
        c300: "#fca5a5".to_string(),
    }
}

fn semantic_success() -> AccentColors {
    AccentColors {
        c600: "#16a34a".to_string(),
        c500: "#22c55e".to_string(),
        c400: "#4ade80".to_string(),
        c300: "#86efac".to_string(),
    }
}

fn semantic_info() -> AccentColors {
    AccentColors {
        c600: "#2563eb".to_string(),
        c500: "#3b82f6".to_string(),
        c400: "#60a5fa".to_string(),
        c300: "#93c5fd".to_string(),
    }
}

fn semantic_warning() -> AccentColors {
    AccentColors {
        c600: "#d97706".to_string(),
        c500: "#f59e0b".to_string(),
        c400: "#fbbf24".to_string(),
        c300: "#fcd34d".to_string(),
    }
}

pub fn normalize_hex(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);

    let expanded = match hex.len() {
        3 => {
            let mut out = String::with_capacity(6);
            for ch in hex.chars() {
                if !ch.is_ascii_hexdigit() {
                    return Err(anyhow::anyhow!("Invalid hex color '{}'", input));
                }
                out.push(ch);
                out.push(ch);
            }
            out
        }
        6 => {
            if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(anyhow::anyhow!("Invalid hex color '{}'", input));
            }
            hex.to_string()
        }
        _ => return Err(anyhow::anyhow!("Invalid hex color '{}'", input)),
    };

    Ok(format!("#{}", expanded.to_ascii_lowercase()))
}

fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    let val = u32::from_str_radix(hex, 16).unwrap_or(0);
    (
        ((val >> 16) & 0xFF) as u8,
        ((val >> 8) & 0xFF) as u8,
        (val & 0xFF) as u8,
    )
}

fn relative_luminance(hex: &str) -> f64 {
    let (r, g, b) = hex_to_rgb(hex);
    let srgb = [r, g, b].map(|v| v as f64 / 255.0);
    let linear = srgb.map(|c| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

fn infer_light_palette_from_text(text_hex: &str) -> bool {
    relative_luminance(text_hex) < 0.45
}

pub fn generate_simple_theme(primary: &str, secondary: &str, text: &str) -> Result<ThemeConfig> {
    let normalized_primary = normalize_hex(primary)?;
    let normalized_secondary = normalize_hex(secondary)?;
    let normalized_text = normalize_hex(text)?;

    let manager = ThemeManager;
    let is_light_palette = infer_light_palette_from_text(&normalized_text);

    let (background, chat_panel, text_muted) = if is_light_palette {
        let bg = manager.interpolate_color(&normalized_text, "#ffffff", 0.96);
        let panel = manager.interpolate_color(&bg, &normalized_secondary, 0.08);
        let muted = manager.interpolate_color(&normalized_text, "#94a3b8", 0.55);
        (bg, panel, muted)
    } else {
        let bg = manager.interpolate_color(&normalized_text, "#020617", 0.92);
        let panel = manager.interpolate_color(&bg, &normalized_secondary, 0.12);
        let muted = manager.interpolate_color(&normalized_text, "#64748b", 0.35);
        (bg, panel, muted)
    };

    let theme = ThemeConfig {
        base: manager.generate_base_colors(&background, &chat_panel, &text_muted, &normalized_text),
        primary: manager.generate_accent_shades(&normalized_primary),
        secondary: manager.generate_accent_shades(&normalized_secondary),
        error: semantic_error(),
        success: semantic_success(),
        info: semantic_info(),
        warning: semantic_warning(),
    };

    validate_and_normalize_theme(&theme)
}

pub fn validate_and_normalize_theme(theme: &ThemeConfig) -> Result<ThemeConfig> {
    Ok(ThemeConfig {
        base: BaseColors {
            c950: normalize_hex(&theme.base.c950)?,
            c900: normalize_hex(&theme.base.c900)?,
            c800: normalize_hex(&theme.base.c800)?,
            c700: normalize_hex(&theme.base.c700)?,
            c600: normalize_hex(&theme.base.c600)?,
            c500: normalize_hex(&theme.base.c500)?,
            c400: normalize_hex(&theme.base.c400)?,
            c300: normalize_hex(&theme.base.c300)?,
            c200: normalize_hex(&theme.base.c200)?,
            c100: normalize_hex(&theme.base.c100)?,
        },
        primary: AccentColors {
            c600: normalize_hex(&theme.primary.c600)?,
            c500: normalize_hex(&theme.primary.c500)?,
            c400: normalize_hex(&theme.primary.c400)?,
            c300: normalize_hex(&theme.primary.c300)?,
        },
        secondary: AccentColors {
            c600: normalize_hex(&theme.secondary.c600)?,
            c500: normalize_hex(&theme.secondary.c500)?,
            c400: normalize_hex(&theme.secondary.c400)?,
            c300: normalize_hex(&theme.secondary.c300)?,
        },
        error: AccentColors {
            c600: normalize_hex(&theme.error.c600)?,
            c500: normalize_hex(&theme.error.c500)?,
            c400: normalize_hex(&theme.error.c400)?,
            c300: normalize_hex(&theme.error.c300)?,
        },
        success: AccentColors {
            c600: normalize_hex(&theme.success.c600)?,
            c500: normalize_hex(&theme.success.c500)?,
            c400: normalize_hex(&theme.success.c400)?,
            c300: normalize_hex(&theme.success.c300)?,
        },
        info: AccentColors {
            c600: normalize_hex(&theme.info.c600)?,
            c500: normalize_hex(&theme.info.c500)?,
            c400: normalize_hex(&theme.info.c400)?,
            c300: normalize_hex(&theme.info.c300)?,
        },
        warning: AccentColors {
            c600: normalize_hex(&theme.warning.c600)?,
            c500: normalize_hex(&theme.warning.c500)?,
            c400: normalize_hex(&theme.warning.c400)?,
            c300: normalize_hex(&theme.warning.c300)?,
        },
    })
}

// ============================================================================
// Theme Preset (simplified JSON schema)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThemePreset {
    pub name: String,
    pub description: String,
    pub background: String,
    #[serde(rename = "chatPanel")]
    pub chat_panel: String,
    #[serde(rename = "primaryAccent")]
    pub primary_accent: String,
    #[serde(rename = "secondaryAccent")]
    pub secondary_accent: String,
    #[serde(rename = "textPrimary")]
    pub text_primary: String,
    #[serde(rename = "textMuted")]
    pub text_muted: String,
}

// ============================================================================
// Embedded Theme Presets (compiled into binary)
// ============================================================================

const EMBEDDED_THEMES: &[(&str, &str)] = &[
    ("arctic_ice", include_str!("../../themes/arctic_ice.json")),
    (
        "cyberpunk_glow",
        include_str!("../../themes/cyberpunk_glow.json"),
    ),
    (
        "earthy_minimal",
        include_str!("../../themes/earthy_minimal.json"),
    ),
    (
        "forest_night",
        include_str!("../../themes/forest_night.json"),
    ),
    (
        "midnight_neon",
        include_str!("../../themes/midnight_neon.json"),
    ),
    (
        "monochrome_pro",
        include_str!("../../themes/monochrome_pro.json"),
    ),
    (
        "ocean_breeze",
        include_str!("../../themes/ocean_breeze.json"),
    ),
    ("rose_noir", include_str!("../../themes/rose_noir.json")),
    ("soft_pastel", include_str!("../../themes/soft_pastel.json")),
    ("solar_flare", include_str!("../../themes/solar_flare.json")),
];

// ============================================================================
// Theme Manager (uses embedded themes)
// ============================================================================

pub struct ThemeManager;

impl ThemeManager {
    pub fn new(_app_dir: &std::path::PathBuf) -> Self {
        Self
    }

    /// List presets with name and description
    pub fn list_presets_info(&self) -> Vec<(String, String, String)> {
        EMBEDDED_THEMES
            .iter()
            .filter_map(|(key, json)| {
                serde_json::from_str::<ThemePreset>(json)
                    .ok()
                    .map(|preset| (key.to_string(), preset.name, preset.description))
            })
            .collect()
    }

    /// Load a preset by name and convert to full ThemeConfig
    pub fn load_preset(&self, name: &str) -> Result<ThemeConfig> {
        let json = EMBEDDED_THEMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, json)| *json)
            .ok_or_else(|| anyhow::anyhow!("Theme preset '{}' not found", name))?;

        let preset: ThemePreset = serde_json::from_str(json)?;
        Ok(self.preset_to_config(&preset))
    }

    /// Convert simplified preset to full ThemeConfig using interpolation
    fn preset_to_config(&self, preset: &ThemePreset) -> ThemeConfig {
        // Generate base colors via interpolation
        let base = self.generate_base_colors(
            &preset.background,
            &preset.chat_panel,
            &preset.text_muted,
            &preset.text_primary,
        );

        // Generate accent shades from primary color
        let primary = self.generate_accent_shades(&preset.primary_accent);
        let secondary = self.generate_accent_shades(&preset.secondary_accent);

        ThemeConfig {
            base,
            primary,
            secondary,
            error: semantic_error(),
            success: semantic_success(),
            info: semantic_info(),
            warning: semantic_warning(),
        }
    }

    fn generate_base_colors(
        &self,
        background: &str,
        chat_panel: &str,
        text_muted: &str,
        text_primary: &str,
    ) -> BaseColors {
        // 950 = background, 900 = chatPanel, 400 = textMuted, 100 = textPrimary
        // Interpolate 800-500 between 900 and 400
        // Interpolate 300-200 between 400 and 100
        BaseColors {
            c950: background.to_string(),
            c900: chat_panel.to_string(),
            c800: self.interpolate_color(chat_panel, text_muted, 0.2),
            c700: self.interpolate_color(chat_panel, text_muted, 0.4),
            c600: self.interpolate_color(chat_panel, text_muted, 0.6),
            c500: self.interpolate_color(chat_panel, text_muted, 0.8),
            c400: text_muted.to_string(),
            c300: self.interpolate_color(text_muted, text_primary, 0.33),
            c200: self.interpolate_color(text_muted, text_primary, 0.66),
            c100: text_primary.to_string(),
        }
    }

    fn generate_accent_shades(&self, base_hex: &str) -> AccentColors {
        let (h, s, _) = self.hex_to_hsl(base_hex);
        AccentColors {
            c600: self.hsl_to_hex(h, (s + 10.0).min(100.0), 40.0),
            c500: self.hsl_to_hex(h, s, 50.0),
            c400: self.hsl_to_hex(h, (s - 5.0).max(0.0), 62.0),
            c300: self.hsl_to_hex(h, (s - 10.0).max(0.0), 75.0),
        }
    }

    fn interpolate_color(&self, c1: &str, c2: &str, factor: f64) -> String {
        let (r1, g1, b1) = self.hex_to_rgb(c1);
        let (r2, g2, b2) = self.hex_to_rgb(c2);
        let r = (r1 as f64 + (r2 as f64 - r1 as f64) * factor).round() as u8;
        let g = (g1 as f64 + (g2 as f64 - g1 as f64) * factor).round() as u8;
        let b = (b1 as f64 + (b2 as f64 - b1 as f64) * factor).round() as u8;
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }

    fn hex_to_rgb(&self, hex: &str) -> (u8, u8, u8) {
        hex_to_rgb(hex)
    }

    fn hex_to_hsl(&self, hex: &str) -> (f64, f64, f64) {
        let (r, g, b) = self.hex_to_rgb(hex);
        let r = r as f64 / 255.0;
        let g = g as f64 / 255.0;
        let b = b as f64 / 255.0;

        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;

        if (max - min).abs() < 0.001 {
            return (0.0, 0.0, l * 100.0);
        }

        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };

        let h = if (max - r).abs() < 0.001 {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
        } else if (max - g).abs() < 0.001 {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };

        (h * 360.0, s * 100.0, l * 100.0)
    }

    fn hsl_to_hex(&self, h: f64, s: f64, l: f64) -> String {
        let s = s / 100.0;
        let l = l / 100.0;
        let a = s * l.min(1.0 - l);

        let f = |n: f64| -> u8 {
            let k = (n + h / 30.0) % 12.0;
            let color = l - a * (k - 3.0).min(9.0 - k).min(1.0).max(-1.0);
            (255.0 * color).round() as u8
        };

        format!("#{:02x}{:02x}{:02x}", f(0.0), f(8.0), f(4.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_theme_infers_dark_palette_for_light_text() {
        let theme = generate_simple_theme("#14b8a6", "#a855f7", "#f8fafc").expect("simple theme");
        assert_eq!(theme.base.c100, "#f8fafc");
        assert!(relative_luminance(&theme.base.c950) < 0.2);
    }

    #[test]
    fn simple_theme_infers_light_palette_for_dark_text() {
        let theme = generate_simple_theme("#14b8a6", "#a855f7", "#0f172a").expect("simple theme");
        assert_eq!(theme.base.c100, "#0f172a");
        assert!(relative_luminance(&theme.base.c950) > 0.8);
    }

    #[test]
    fn normalize_hex_accepts_short_form() {
        assert_eq!(normalize_hex("#abc").expect("hex"), "#aabbcc");
    }

    #[test]
    fn validate_theme_rejects_bad_hex() {
        let mut theme = ThemeConfig::default();
        theme.primary.c500 = "not-a-color".to_string();
        assert!(validate_and_normalize_theme(&theme).is_err());
    }
}
