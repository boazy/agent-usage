struct StyleOption {
    name: &'static str,
    chars: &'static str,
}

fn main() {
    let styles = [
        StyleOption {
            name: "classic",
            chars: "█▉▊",
        },
        StyleOption {
            name: "soft",
            chars: "█▓▒",
        },
        StyleOption {
            name: "squares",
            chars: "█▓▌",
        },
        StyleOption {
            name: "ascii",
            chars: "#=-",
        },
        StyleOption {
            name: "braille",
            chars: "⣿⣶⣤",
        },
    ];

    let samples = [0u64, 12u64, 42u64, 77u64, 100u64];

    for style in styles {
        println!("style: {} ({})", style.name, style.chars);
        for percent in samples {
            render_bar(style.chars, percent);
        }
        println!();
    }
}

fn render_bar(chars: &str, percent: u64) {
    let width = 20;
    let chars: Vec<char> = chars.chars().collect();
    let full = chars.first().copied().unwrap_or('█');
    let partial_one = chars.get(1).copied().unwrap_or(full);
    let partial_two = chars.get(2).copied().unwrap_or(partial_one);
    let empty = ' ';

    let filled_units = ((percent as f64 / 100.0) * width as f64).clamp(0.0, width as f64);
    let whole_units = filled_units.floor() as usize;
    let remainder = filled_units - whole_units as f64;

    let mut bar = String::with_capacity(width);
    for idx in 0..width {
        let glyph = if idx < whole_units {
            full
        } else if idx == whole_units && remainder > 0.0 {
            if remainder < 0.33 {
                partial_one
            } else if remainder < 0.67 {
                partial_two
            } else {
                full
            }
        } else {
            empty
        };

        bar.push(glyph);
    }

    println!("  {percent:>3}% |{bar}|", percent = percent, bar = bar);
}
