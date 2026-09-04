use anstyle::{Ansi256Color, AnsiColor, Color, RgbColor, Style};

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) name: &'static str,
    pub(crate) warning: Style,
    pub(crate) exhausted: Style,
    pub(crate) unknown: Style,
    pub(crate) bar_primary: Style,
    pub(crate) bar_background: Style,
    pub(crate) meter: Style,
    pub(crate) window: Style,
    pub(crate) reset: Style,
}

const fn indexed(index: u8) -> Style {
    Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(index))))
}

const fn rgb(red: u8, green: u8, blue: u8) -> Style {
    Style::new().fg_color(Some(Color::Rgb(RgbColor(red, green, blue))))
}

const fn ansi(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(color)))
}

pub(crate) const BUILTIN_THEMES: &[Theme] = &[
    Theme {
        name: "default",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Blue),
        meter: indexed(39),
        window: indexed(243),
        reset: indexed(244),
        bar_primary: rgb(63, 185, 80),
        bar_background: rgb(41, 92, 50),
    },
    Theme {
        name: "solarized-dark",
        warning: ansi(AnsiColor::BrightYellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::BrightBlue),
        meter: indexed(75),
        window: indexed(144),
        reset: indexed(245),
        bar_primary: rgb(42, 161, 152),
        bar_background: rgb(24, 88, 84),
    },
    Theme {
        name: "solarized-light",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Cyan),
        meter: indexed(33),
        window: indexed(101),
        reset: indexed(244),
        bar_primary: rgb(38, 139, 210),
        bar_background: rgb(190, 213, 233),
    },
    Theme {
        name: "monokai",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Magenta),
        meter: indexed(77),
        window: indexed(180),
        reset: indexed(244),
        bar_primary: rgb(166, 226, 46),
        bar_background: rgb(85, 102, 34),
    },
    Theme {
        name: "molokai",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Magenta),
        meter: indexed(148),
        window: indexed(244),
        reset: indexed(245),
        bar_primary: rgb(184, 230, 62),
        bar_background: rgb(92, 104, 40),
    },
    Theme {
        name: "dracula",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::BrightCyan),
        meter: indexed(141),
        window: indexed(103),
        reset: indexed(243),
        bar_primary: rgb(189, 147, 249),
        bar_background: rgb(88, 70, 120),
    },
    Theme {
        name: "gruvbox-dark",
        warning: ansi(AnsiColor::BrightYellow),
        exhausted: ansi(AnsiColor::BrightRed),
        unknown: ansi(AnsiColor::Cyan),
        meter: indexed(214),
        window: indexed(180),
        reset: indexed(244),
        bar_primary: rgb(250, 189, 47),
        bar_background: rgb(110, 88, 24),
    },
    Theme {
        name: "gruvbox-light",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Magenta),
        meter: indexed(66),
        window: indexed(59),
        reset: indexed(244),
        bar_primary: rgb(7, 102, 120),
        bar_background: rgb(180, 205, 211),
    },
    Theme {
        name: "one-dark",
        warning: ansi(AnsiColor::BrightYellow),
        exhausted: ansi(AnsiColor::BrightRed),
        unknown: ansi(AnsiColor::BrightCyan),
        meter: indexed(114),
        window: indexed(181),
        reset: indexed(245),
        bar_primary: rgb(152, 195, 121),
        bar_background: rgb(70, 95, 60),
    },
    Theme {
        name: "one-light",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Blue),
        meter: indexed(28),
        window: indexed(101),
        reset: indexed(244),
        bar_primary: rgb(80, 161, 79),
        bar_background: rgb(196, 220, 196),
    },
    Theme {
        name: "nord",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Blue),
        meter: indexed(74),
        window: indexed(104),
        reset: indexed(244),
        bar_primary: rgb(136, 192, 208),
        bar_background: rgb(62, 90, 99),
    },
    Theme {
        name: "github-dark",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::BrightRed),
        unknown: ansi(AnsiColor::Blue),
        meter: indexed(47),
        window: indexed(249),
        reset: indexed(240),
        bar_primary: rgb(63, 185, 80),
        bar_background: rgb(40, 86, 48),
    },
    Theme {
        name: "github-light",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Cyan),
        meter: indexed(24),
        window: indexed(244),
        reset: indexed(244),
        bar_primary: rgb(9, 105, 218),
        bar_background: rgb(200, 216, 240),
    },
    Theme {
        name: "nord-dark",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Magenta),
        meter: indexed(117),
        window: indexed(152),
        reset: indexed(245),
        bar_primary: rgb(94, 129, 172),
        bar_background: rgb(52, 72, 97),
    },
    Theme {
        name: "catppuccin-mocha",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::BrightRed),
        unknown: ansi(AnsiColor::Cyan),
        meter: indexed(141),
        window: indexed(103),
        reset: indexed(243),
        bar_primary: rgb(203, 166, 247),
        bar_background: rgb(90, 74, 114),
    },
    Theme {
        name: "tokyo-night",
        warning: ansi(AnsiColor::Yellow),
        exhausted: ansi(AnsiColor::Red),
        unknown: ansi(AnsiColor::Magenta),
        meter: indexed(110),
        window: indexed(246),
        reset: indexed(242),
        bar_primary: rgb(122, 162, 247),
        bar_background: rgb(58, 78, 116),
    },
];

pub(crate) fn available_theme_names() -> String {
    BUILTIN_THEMES
        .iter()
        .map(|theme| theme.name)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn theme_by_name(name: &str) -> Option<Theme> {
    BUILTIN_THEMES
        .iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name))
        .copied()
}
