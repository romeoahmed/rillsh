//! Terminal hints select presentation only; they never change language behavior.

/// Explicit command-line color policy, independent of cursor-based editing.
#[derive(Clone, Copy)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

/// The maximum color vocabulary advertised by an output destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorDepth {
    Plain,
    Ansi16,
    Ansi256,
    TrueColor,
}
impl ColorDepth {
    #[must_use]
    pub const fn enabled(self) -> bool {
        !matches!(self, Self::Plain)
    }

    pub(crate) const fn select(
        self,
        basic: nu_ansi_term::Color,
        indexed: u8,
        rgb: (u8, u8, u8),
    ) -> nu_ansi_term::Color {
        match self {
            Self::Plain => nu_ansi_term::Color::Default,
            Self::Ansi16 => basic,
            Self::Ansi256 => nu_ansi_term::Color::Fixed(indexed),
            Self::TrueColor => nu_ansi_term::Color::Rgb(rgb.0, rgb.1, rgb.2),
        }
    }
}

/// Borrowed session environment; color depth follows explicit capability hints.
#[derive(Default)]
pub struct Hints<'a> {
    pub term: &'a [u8],
    pub colorterm: &'a [u8],
    pub no_color: &'a [u8],
}
impl Hints<'_> {
    /// Use the native editor unless TERM is absent or explicitly requests plain output.
    #[must_use]
    pub const fn supports_ansi(&self) -> bool {
        !matches!(self.term, b"" | b"dumb")
    }

    #[must_use]
    pub fn color(&self, choice: ColorChoice, is_terminal: bool) -> ColorDepth {
        match choice {
            ColorChoice::Never => return ColorDepth::Plain,
            ColorChoice::Auto
                if !is_terminal || !self.supports_ansi() || !self.no_color.is_empty() =>
            {
                return ColorDepth::Plain;
            }
            ColorChoice::Auto | ColorChoice::Always => {}
        }
        if matches!(self.colorterm, b"truecolor" | b"24bit") || self.term.ends_with(b"-direct") {
            ColorDepth::TrueColor
        } else if self.term.ends_with(b"-256color") {
            ColorDepth::Ansi256
        } else {
            ColorDepth::Ansi16
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_and_cursor_capabilities_are_independent() {
        for term in [b"".as_slice(), b"dumb"] {
            let hints = Hints {
                term,
                ..Hints::default()
            };
            assert!(!hints.supports_ansi());
            assert_eq!(hints.color(ColorChoice::Auto, true), ColorDepth::Plain);
            assert_eq!(hints.color(ColorChoice::Always, false), ColorDepth::Ansi16);
        }
        for (term, colorterm, expected) in [
            (b"custom-vt".as_slice(), b"".as_slice(), ColorDepth::Ansi16),
            (b"xterm-256color", b"", ColorDepth::Ansi256),
            (b"tmux-256color", b"truecolor", ColorDepth::TrueColor),
            (b"foot", b"24bit", ColorDepth::TrueColor),
            (b"xterm-direct", b"", ColorDepth::TrueColor),
        ] {
            let mut hints = Hints {
                term,
                colorterm,
                no_color: b"",
            };
            assert!(hints.supports_ansi());
            assert_eq!(hints.color(ColorChoice::Auto, true), expected);
            assert_eq!(hints.color(ColorChoice::Auto, false), ColorDepth::Plain);
            assert_eq!(hints.color(ColorChoice::Never, true), ColorDepth::Plain);
            hints.no_color = b"1";
            assert_eq!(hints.color(ColorChoice::Auto, true), ColorDepth::Plain);
            assert_eq!(hints.color(ColorChoice::Always, false), expected);
        }
    }
}
