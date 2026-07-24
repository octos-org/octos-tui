use ratatui::style::{Color, Style};

use crate::cli::ThemeName;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub surface: Color,
    pub surface_alt: Color,
    pub frame: Color,
    pub accent: Color,
    pub highlight: Color,
    pub text: Color,
    pub muted: Color,
    pub success: Color,
    pub success_bg: Color,
    pub danger: Color,
    pub danger_bg: Color,
    pub diff_context_bg: Color,
    /// syntect theme (from the default theme set) used for fenced-code
    /// highlighting under this UI theme. Foreground-only at render time.
    pub code_theme: &'static str,
}

impl Palette {
    pub fn for_theme(theme: ThemeName) -> Self {
        match theme {
            ThemeName::Terminal => {
                let light = crate::terminal_probe::terminal_info().is_light_bg();
                Self {
                    surface: Color::Reset,
                    surface_alt: Color::Reset,
                    frame: if light {
                        Color::DarkGray
                    } else {
                        Color::Gray
                    },
                    accent: Color::Cyan,
                    highlight: Color::Yellow,
                    text: Color::Reset,
                    muted: if light {
                        Color::DarkGray
                    } else {
                        Color::Gray
                    },
                    success: Color::Cyan,
                    success_bg: Color::Reset,
                    danger: Color::Red,
                    danger_bg: Color::Reset,
                    diff_context_bg: Color::Reset,
                    code_theme: if light {
                        "base16-github.light"
                    } else {
                        "base16-eighties.dark"
                    },
                }
            }
            ThemeName::Slate => Self {
                surface: Color::Rgb(20, 25, 35),
                surface_alt: Color::Rgb(28, 34, 46),
                frame: Color::Rgb(48, 57, 73),
                accent: Color::Rgb(99, 151, 255),
                highlight: Color::Rgb(246, 199, 94),
                text: Color::Rgb(230, 236, 242),
                muted: Color::Rgb(145, 156, 170),
                success: Color::Rgb(91, 196, 129),
                success_bg: Color::Rgb(22, 48, 36),
                danger: Color::Rgb(232, 95, 95),
                danger_bg: Color::Rgb(58, 28, 32),
                diff_context_bg: Color::Rgb(24, 31, 43),
                code_theme: "base16-ocean.dark",
            },
            ThemeName::Codex => Self {
                surface: Color::Rgb(15, 18, 24),
                surface_alt: Color::Rgb(26, 30, 39),
                frame: Color::Rgb(90, 94, 108),
                accent: Color::Rgb(110, 188, 255),
                highlight: Color::Rgb(255, 209, 102),
                text: Color::Rgb(236, 239, 244),
                muted: Color::Rgb(154, 162, 175),
                success: Color::Rgb(104, 211, 145),
                success_bg: Color::Rgb(18, 50, 34),
                danger: Color::Rgb(248, 113, 113),
                danger_bg: Color::Rgb(64, 27, 32),
                diff_context_bg: Color::Rgb(22, 26, 34),
                code_theme: "base16-eighties.dark",
            },
            ThemeName::Claude => Self {
                surface: Color::Rgb(38, 31, 26),
                surface_alt: Color::Rgb(54, 44, 36),
                frame: Color::Rgb(92, 78, 65),
                accent: Color::Rgb(242, 143, 93),
                highlight: Color::Rgb(126, 210, 166),
                text: Color::Rgb(244, 241, 234),
                muted: Color::Rgb(174, 164, 150),
                success: Color::Rgb(120, 205, 150),
                success_bg: Color::Rgb(37, 58, 43),
                danger: Color::Rgb(235, 111, 106),
                danger_bg: Color::Rgb(70, 38, 34),
                diff_context_bg: Color::Rgb(45, 37, 31),
                code_theme: "base16-mocha.dark",
            },
            ThemeName::Solarized => Self {
                surface: Color::Rgb(0, 43, 54),
                surface_alt: Color::Rgb(7, 54, 66),
                frame: Color::Rgb(88, 110, 117),
                accent: Color::Rgb(38, 139, 210),
                highlight: Color::Rgb(181, 137, 0),
                text: Color::Rgb(238, 232, 213),
                muted: Color::Rgb(147, 161, 161),
                success: Color::Rgb(133, 153, 0),
                success_bg: Color::Rgb(17, 67, 48),
                danger: Color::Rgb(220, 50, 47),
                danger_bg: Color::Rgb(75, 44, 48),
                diff_context_bg: Color::Rgb(5, 50, 61),
                code_theme: "Solarized (dark)",
            },
        }
    }

    pub fn border(self) -> Style {
        Style::default().fg(self.frame)
    }

    pub fn title(self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn selected(self) -> Style {
        Style::default().fg(self.highlight)
    }

    pub fn text(self) -> Style {
        Style::default().fg(self.text)
    }

    pub fn muted(self) -> Style {
        Style::default().fg(self.muted)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::Palette;
    use crate::cli::ThemeName;

    #[test]
    fn terminal_theme_uses_terminal_default_surfaces() {
        let palette = Palette::for_theme(ThemeName::Terminal);

        assert_eq!(palette.surface, Color::Reset);
        assert_eq!(palette.surface_alt, Color::Reset);
        assert_eq!(palette.text, Color::Reset);
        assert_eq!(palette.diff_context_bg, Color::Reset);
        assert_eq!(palette.success_bg, Color::Reset);
        assert_eq!(palette.danger_bg, Color::Reset);
    }

    #[test]
    fn terminal_theme_muted_depends_on_detected_bg() {
        // Terminal theme adapts muted/frame to the detected background.
        // We can't control the probe in tests, so just verify the palette
        // is constructed (no panic) and has valid colors.
        let palette = Palette::for_theme(ThemeName::Terminal);
        assert_ne!(palette.accent, Color::Reset);
        assert_ne!(palette.highlight, Color::Reset);
    }

    #[test]
    fn terminal_theme_avoids_forced_green_highlights() {
        let palette = Palette::for_theme(ThemeName::Terminal);

        assert_ne!(palette.highlight, Color::Green);
        assert_ne!(palette.highlight, Color::LightGreen);
        assert_ne!(palette.success, Color::Green);
        assert_ne!(palette.success, Color::LightGreen);
    }
}
