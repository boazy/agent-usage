use anstyle::{Color, RgbColor, Style};

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) name: &'static str,
    pub(crate) title: Style,
    pub(crate) border: Style,
    pub(crate) active_border: Style,
    pub(crate) selection: Style,
    pub(crate) label: Style,
    pub(crate) source: Style,
    pub(crate) plan: Style,
    pub(crate) account: Style,
    pub(crate) warning: Style,
    pub(crate) exhausted: Style,
    pub(crate) unknown: Style,
    pub(crate) bar_primary: Style,
    pub(crate) bar_background: Style,
    pub(crate) meter: Style,
    pub(crate) window: Style,
    pub(crate) reset_label: Style,
    pub(crate) reset_time: Style,
}

const fn rgb(hex: u32) -> Style {
    let [_, red, green, blue] = hex.to_be_bytes();
    Style::new().fg_color(Some(Color::Rgb(RgbColor(red, green, blue))))
}

#[derive(Clone, Copy)]
struct Palette {
    accent: u32,
    secondary: u32,
    tertiary: u32,
    muted: u32,
    border: u32,
    primary: u32,
    bar_muted: u32,
    warning: u32,
    error: u32,
}

// Like Yazi's theme roles, structural borders, active pickers and content each
// have their own role. Palette RGB values never depend on terminal ANSI slots.
const fn theme(name: &'static str, palette: Palette) -> Theme {
    Theme {
        name,
        title: rgb(palette.accent).bold(),
        border: rgb(palette.border),
        active_border: rgb(palette.accent),
        selection: rgb(palette.secondary).bold().underline(),
        label: rgb(palette.muted),
        source: rgb(palette.tertiary),
        plan: rgb(palette.secondary),
        account: rgb(palette.accent),
        warning: rgb(palette.warning),
        exhausted: rgb(palette.error),
        unknown: rgb(palette.muted).italic(),
        bar_primary: rgb(palette.primary),
        bar_background: rgb(palette.bar_muted),
        meter: rgb(palette.accent).bold(),
        window: rgb(palette.muted).italic(),
        reset_label: rgb(palette.muted).italic(),
        reset_time: rgb(palette.tertiary),
    }
}

pub(crate) const BUILTIN_THEMES: &[Theme] = &[
    theme(
        "default",
        Palette {
            accent: 0x0079_c0ff,
            secondary: 0x00d2_a8ff,
            tertiary: 0x007e_e787,
            muted: 0x008b_acc7,
            border: 0x0048_627a,
            primary: 0x003f_b950,
            bar_muted: 0x0029_5c32,
            warning: 0x00e3_b341,
            error: 0x00ff_7b72,
        },
    ),
    theme(
        "solarized-dark",
        Palette {
            accent: 0x0026_8bd2,
            secondary: 0x006c_71c4,
            tertiary: 0x002a_a198,
            muted: 0x0083_9c8f,
            border: 0x0058_6e75,
            primary: 0x002a_a198,
            bar_muted: 0x0018_5854,
            warning: 0x00b5_8900,
            error: 0x00dc_322f,
        },
    ),
    theme(
        "solarized-light",
        Palette {
            accent: 0x0026_8bd2,
            secondary: 0x006c_71c4,
            tertiary: 0x002a_a198,
            muted: 0x0065_7b73,
            border: 0x0093_a1a1,
            primary: 0x0026_8bd2,
            bar_muted: 0x00be_d5e9,
            warning: 0x00b5_8900,
            error: 0x00dc_322f,
        },
    ),
    theme(
        "monokai",
        Palette {
            accent: 0x00a6_e22e,
            secondary: 0x00ae_81ff,
            tertiary: 0x0066_d9ef,
            muted: 0x00b5_ad83,
            border: 0x0075_715e,
            primary: 0x00a6_e22e,
            bar_muted: 0x0055_6622,
            warning: 0x00e6_db74,
            error: 0x00f9_2672,
        },
    ),
    theme(
        "molokai",
        Palette {
            accent: 0x00fd_971f,
            secondary: 0x00ae_81ff,
            tertiary: 0x0066_d9ef,
            muted: 0x00b0_ac91,
            border: 0x0075_715e,
            primary: 0x00b8_e63e,
            bar_muted: 0x005c_6828,
            warning: 0x00e6_db74,
            error: 0x00f9_2672,
        },
    ),
    theme(
        "dracula",
        Palette {
            accent: 0x00bd_93f9,
            secondary: 0x00ff_79c6,
            tertiary: 0x008b_e9fd,
            muted: 0x009d_9dcc,
            border: 0x0062_72a4,
            primary: 0x00bd_93f9,
            bar_muted: 0x0058_4678,
            warning: 0x00f1_fa8c,
            error: 0x00ff_5555,
        },
    ),
    theme(
        "gruvbox-dark",
        Palette {
            accent: 0x00fa_bd2f,
            secondary: 0x00d3_869b,
            tertiary: 0x008e_c07c,
            muted: 0x00bd_ae93,
            border: 0x007c_6f64,
            primary: 0x00fa_bd2f,
            bar_muted: 0x006e_5818,
            warning: 0x00fe_8019,
            error: 0x00fb_4934,
        },
    ),
    theme(
        "gruvbox-light",
        Palette {
            accent: 0x0007_6678,
            secondary: 0x008f_3f71,
            tertiary: 0x0042_7b58,
            muted: 0x007c_6f64,
            border: 0x00a8_9984,
            primary: 0x0007_6678,
            bar_muted: 0x00b4_cdd3,
            warning: 0x00af_3a03,
            error: 0x009d_0006,
        },
    ),
    theme(
        "one-dark",
        Palette {
            accent: 0x0061_afef,
            secondary: 0x00c6_78dd,
            tertiary: 0x0056_b6c2,
            muted: 0x009c_a3ba,
            border: 0x005c_6370,
            primary: 0x0098_c379,
            bar_muted: 0x0046_5f3c,
            warning: 0x00e5_c07b,
            error: 0x00e0_6c75,
        },
    ),
    theme(
        "one-light",
        Palette {
            accent: 0x0040_78f2,
            secondary: 0x00a6_26a4,
            tertiary: 0x0001_84bc,
            muted: 0x0069_6c83,
            border: 0x00a0_a1a7,
            primary: 0x0050_a14f,
            bar_muted: 0x00c4_dcc4,
            warning: 0x0098_6801,
            error: 0x00e4_5649,
        },
    ),
    theme(
        "nord",
        Palette {
            accent: 0x0088_c0d0,
            secondary: 0x00b4_8ead,
            tertiary: 0x008f_bcbb,
            muted: 0x0092_a4be,
            border: 0x004c_566a,
            primary: 0x0088_c0d0,
            bar_muted: 0x003e_5a63,
            warning: 0x00eb_cb8b,
            error: 0x00bf_616a,
        },
    ),
    theme(
        "github-dark",
        Palette {
            accent: 0x0058_a6ff,
            secondary: 0x00bc_8cff,
            tertiary: 0x0056_d364,
            muted: 0x008b_a6bf,
            border: 0x0048_4f58,
            primary: 0x003f_b950,
            bar_muted: 0x0028_5630,
            warning: 0x00d2_9922,
            error: 0x00f8_5149,
        },
    ),
    theme(
        "github-light",
        Palette {
            accent: 0x0009_69da,
            secondary: 0x0082_50df,
            tertiary: 0x001a_7f37,
            muted: 0x0062_768b,
            border: 0x00af_b8c1,
            primary: 0x0009_69da,
            bar_muted: 0x00c8_d8f0,
            warning: 0x009a_6700,
            error: 0x00cf_222e,
        },
    ),
    theme(
        "nord-dark",
        Palette {
            accent: 0x0081_a1c1,
            secondary: 0x00b4_8ead,
            tertiary: 0x008f_bcbb,
            muted: 0x008d_a0bb,
            border: 0x0043_4c5e,
            primary: 0x005e_81ac,
            bar_muted: 0x0034_4861,
            warning: 0x00eb_cb8b,
            error: 0x00bf_616a,
        },
    ),
    theme(
        "catppuccin-mocha",
        Palette {
            accent: 0x00cb_a6f7,
            secondary: 0x00f5_c2e7,
            tertiary: 0x0094_e2d5,
            muted: 0x00a6_adc8,
            border: 0x0058_5b70,
            primary: 0x00cb_a6f7,
            bar_muted: 0x005a_4a72,
            warning: 0x00f9_e2af,
            error: 0x00f3_8ba8,
        },
    ),
    theme(
        "tokyo-night",
        Palette {
            accent: 0x007a_a2f7,
            secondary: 0x00bb_9af7,
            tertiary: 0x007d_cfff,
            muted: 0x0089_9ac4,
            border: 0x0041_4868,
            primary: 0x007a_a2f7,
            bar_muted: 0x003a_4e74,
            warning: 0x00e0_af68,
            error: 0x00f7_768e,
        },
    ),
    theme(
        "everforest",
        Palette {
            accent: 0x00a7_c080,
            secondary: 0x00d6_99b6,
            tertiary: 0x0083_c092,
            muted: 0x009d_a9a0,
            border: 0x0056_635f,
            primary: 0x00a7_c080,
            bar_muted: 0x0049_5b45,
            warning: 0x00db_bc7f,
            error: 0x00e6_7e80,
        },
    ),
    theme(
        "kanagawa",
        Palette {
            accent: 0x007e_9cd8,
            secondary: 0x0095_7fb8,
            tertiary: 0x007a_a89f,
            muted: 0x009c_abca,
            border: 0x0054_546d,
            primary: 0x0098_bb6c,
            bar_muted: 0x0048_5442,
            warning: 0x00e6_c384,
            error: 0x00c3_4043,
        },
    ),
    theme(
        "rose-pine",
        Palette {
            accent: 0x00c4_a7e7,
            secondary: 0x00eb_bcba,
            tertiary: 0x009c_cfd8,
            muted: 0x0090_8caa,
            border: 0x0052_4f67,
            primary: 0x00eb_bcba,
            bar_muted: 0x006e_515b,
            warning: 0x00f6_c177,
            error: 0x00eb_6f92,
        },
    ),
    theme(
        "rose-pine-dawn",
        Palette {
            accent: 0x0090_7aa9,
            secondary: 0x00b4_637a,
            tertiary: 0x0028_6983,
            muted: 0x0079_7593,
            border: 0x00b2_a3b4,
            primary: 0x00d7_827e,
            bar_muted: 0x00ea_d2d0,
            warning: 0x00b8_791a,
            error: 0x00b4_637a,
        },
    ),
    theme(
        "ayu-dark",
        Palette {
            accent: 0x00ff_b454,
            secondary: 0x00d2_a6ff,
            tertiary: 0x0095_e6cb,
            muted: 0x008a_9baf,
            border: 0x0046_5366,
            primary: 0x00e6_b450,
            bar_muted: 0x0066_552f,
            warning: 0x00ff_8f40,
            error: 0x00f0_7178,
        },
    ),
    theme(
        "catppuccin-latte",
        Palette {
            accent: 0x0088_39ef,
            secondary: 0x00ea_76cb,
            tertiary: 0x0017_9299,
            muted: 0x006c_6f85,
            border: 0x009c_a0b0,
            primary: 0x0040_a02b,
            bar_muted: 0x00b9_d6b3,
            warning: 0x00df_8e1d,
            error: 0x00d2_0f39,
        },
    ),
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
