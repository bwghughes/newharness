mod agent;
mod client;
mod spinner;
mod tools;
mod types;

use agent::Agent;
use client::LlmClient;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn lerp(&self, other: &Rgb, t: f64) -> Rgb {
        Rgb(
            (self.0 as f64 + (other.0 as f64 - self.0 as f64) * t) as u8,
            (self.1 as f64 + (other.1 as f64 - self.1 as f64) * t) as u8,
            (self.2 as f64 + (other.2 as f64 - self.2 as f64) * t) as u8,
        )
    }

    fn dim(&self, factor: f64) -> Rgb {
        Rgb(
            (self.0 as f64 * factor) as u8,
            (self.1 as f64 * factor) as u8,
            (self.2 as f64 * factor) as u8,
        )
    }
}

enum ColorMode {
    TrueColor,
    Ansi256,
    Basic,
}

fn detect_color_mode() -> ColorMode {
    if let Ok(ct) = std::env::var("COLORTERM") {
        if ct == "truecolor" || ct == "24bit" {
            return ColorMode::TrueColor;
        }
    }
    if let Ok(tp) = std::env::var("TERM_PROGRAM") {
        match tp.to_lowercase().as_str() {
            "ghostty" | "iterm.app" | "wezterm" | "kitty" | "alacritty" => {
                return ColorMode::TrueColor;
            }
            _ => {}
        }
    }
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("256color") {
            return ColorMode::Ansi256;
        }
    }
    ColorMode::Basic
}

fn supports_styled_underlines() -> bool {
    std::env::var("TERM_PROGRAM")
        .map(|tp| matches!(tp.to_lowercase().as_str(), "ghostty" | "kitty" | "wezterm"))
        .unwrap_or(false)
}

fn spectrum(t: f64, stops: &[Rgb]) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let segment = t * (stops.len() - 1) as f64;
    let idx = (segment as usize).min(stops.len() - 2);
    let local_t = segment - idx as f64;
    stops[idx].lerp(&stops[idx + 1], local_t)
}

fn neon_spectrum(t: f64) -> Rgb {
    spectrum(
        t,
        &[
            Rgb(255, 16, 120),
            Rgb(255, 0, 200),
            Rgb(180, 40, 255),
            Rgb(80, 80, 255),
            Rgb(0, 160, 255),
            Rgb(0, 255, 200),
        ],
    )
}

const BANNER_ART: [&str; 6] = [
    r"  _____ _______ _____            _____    _____ _   _  ",
    r" / ____|__   __|  __ \     /\   |  __ \  |_   _| \ | | ",
    r"| (___    | |  | |__) |   /  \  | |__) |   | | |  \| | ",
    r" \___ \   | |  |  _  /   / /\ \ |  ___/    | | | . ` | ",
    r" ____) |  | |  | | \ \  / ____ \| |       _| |_| |\  | ",
    r"|_____/   |_|  |_|  \_\/_/    \_\_|      |_____|_| \_| ",
];

fn write_separator(mode: &ColorMode) {
    let mut stderr = io::stderr().lock();
    match mode {
        ColorMode::TrueColor => {
            let chars: Vec<char> = "✦ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ ✦"
                .chars()
                .collect();
            let _ = write!(stderr, "  ");
            for (i, &ch) in chars.iter().enumerate() {
                if ch == ' ' {
                    let _ = write!(stderr, " ");
                } else {
                    let t = i as f64 / chars.len() as f64;
                    let Rgb(r, g, b) = neon_spectrum(t).dim(0.35);
                    let _ = write!(stderr, "\x1b[38;2;{r};{g};{b}m{ch}");
                }
            }
            let _ = writeln!(stderr, "\x1b[0m");
        }
        ColorMode::Ansi256 => {
            let _ = writeln!(
                stderr,
                "  \x1b[38;5;240m* ---------------------------------------------------- *\x1b[0m"
            );
        }
        ColorMode::Basic => {
            let _ = writeln!(
                stderr,
                "  \x1b[2m* ---------------------------------------------------- *\x1b[0m"
            );
        }
    }
}

fn write_art(mode: &ColorMode, fancy_underline: bool) {
    let mut stderr = io::stderr().lock();
    match mode {
        ColorMode::TrueColor => {
            let total = BANNER_ART.len();
            for (row, line) in BANNER_ART.iter().enumerate() {
                let row_t = row as f64 / (total - 1) as f64;
                let start = neon_spectrum(row_t);
                let end = neon_spectrum((row_t + 0.2).min(1.0));
                let chars: Vec<char> = line.chars().collect();
                let width = chars.len().max(1);

                for (col, &ch) in chars.iter().enumerate() {
                    if ch == ' ' {
                        let _ = write!(stderr, "\x1b[0m ");
                        continue;
                    }
                    let col_t = col as f64 / width as f64;
                    let color = start.lerp(&end, col_t);
                    let Rgb(r, g, b) = color;
                    if fancy_underline {
                        let glow = color.dim(0.45);
                        let _ = write!(
                            stderr,
                            "\x1b[1;38;2;{r};{g};{b}m\x1b[4:3m\x1b[58;2;{};{};{}m{ch}",
                            glow.0, glow.1, glow.2
                        );
                    } else {
                        let _ = write!(stderr, "\x1b[1;38;2;{r};{g};{b}m{ch}");
                    }
                }
                let _ = writeln!(stderr, "\x1b[0m");
            }
        }
        ColorMode::Ansi256 => {
            let colors: [u8; 6] = [197, 163, 129, 69, 39, 49];
            for (row, line) in BANNER_ART.iter().enumerate() {
                let _ = writeln!(stderr, "\x1b[1;38;5;{}m{line}\x1b[0m", colors[row]);
            }
        }
        ColorMode::Basic => {
            let escapes: [&str; 6] = [
                "\x1b[1;35m",
                "\x1b[1;31m",
                "\x1b[1;34m",
                "\x1b[1;36m",
                "\x1b[1;32m",
                "\x1b[1;33m",
            ];
            for (row, line) in BANNER_ART.iter().enumerate() {
                let _ = writeln!(stderr, "{}{line}\x1b[0m", escapes[row]);
            }
        }
    }
}

fn write_tagline(mode: &ColorMode) {
    let mut stderr = io::stderr().lock();
    match mode {
        ColorMode::TrueColor => {
            let prefix = "  Fast coding agent harness";
            for (i, ch) in prefix.chars().enumerate() {
                if ch == ' ' {
                    let _ = write!(stderr, " ");
                } else {
                    let t = i as f64 / prefix.len() as f64;
                    let Rgb(r, g, b) = neon_spectrum(t * 0.3).dim(0.7);
                    let _ = write!(stderr, "\x1b[38;2;{r};{g};{b}m{ch}");
                }
            }
            let Rgb(r, g, b) = neon_spectrum(0.5);
            let _ = write!(stderr, "\x1b[0m \x1b[1;38;2;{r};{g};{b}m—\x1b[0m");
            let suffix = " any OpenAI-compatible endpoint";
            for (i, ch) in suffix.chars().enumerate() {
                if ch == ' ' {
                    let _ = write!(stderr, " ");
                } else {
                    let t = 0.5 + (i as f64 / suffix.len() as f64) * 0.5;
                    let Rgb(r, g, b) = neon_spectrum(t).dim(0.5);
                    let _ = write!(stderr, "\x1b[38;2;{r};{g};{b}m{ch}");
                }
            }
            let _ = writeln!(stderr, "\x1b[0m");
        }
        ColorMode::Ansi256 => {
            let _ = writeln!(
                stderr,
                "  \x1b[2mFast coding agent harness \x1b[0m\x1b[1;38;5;197m—\x1b[0m\x1b[2m any OpenAI-compatible endpoint\x1b[0m"
            );
        }
        ColorMode::Basic => {
            let _ = writeln!(
                stderr,
                "  \x1b[2mFast coding agent harness \x1b[1;35m—\x1b[0m\x1b[2m any OpenAI-compatible endpoint\x1b[0m"
            );
        }
    }
}

fn write_hints(mode: &ColorMode) {
    let mut stderr = io::stderr().lock();
    match mode {
        ColorMode::TrueColor => {
            let Rgb(pr, pg, pb) = neon_spectrum(0.4);
            let Rgb(cr, cg, cb) = neon_spectrum(0.8);
            let _ = writeln!(
                stderr,
                "  \x1b[2mType your request, or \x1b[0m\x1b[38;2;{pr};{pg};{pb}m'quit'\x1b[0m\x1b[2m to exit.\x1b[0m"
            );
            let _ = writeln!(
                stderr,
                "  \x1b[2mCommands: \x1b[0m\x1b[38;2;{cr};{cg};{cb}m/compact\x1b[0m\x1b[2m, \x1b[0m\x1b[38;2;{cr};{cg};{cb}m/clear\x1b[0m"
            );
        }
        ColorMode::Ansi256 => {
            let _ = writeln!(
                stderr,
                "  \x1b[2mType your request, or \x1b[0m\x1b[38;5;129m'quit'\x1b[0m\x1b[2m to exit.\x1b[0m"
            );
            let _ = writeln!(
                stderr,
                "  \x1b[2mCommands: \x1b[0m\x1b[38;5;44m/compact\x1b[0m\x1b[2m, \x1b[0m\x1b[38;5;44m/clear\x1b[0m"
            );
        }
        ColorMode::Basic => {
            let _ = writeln!(
                stderr,
                "  \x1b[2mType your request, or \x1b[0m\x1b[35m'quit'\x1b[0m\x1b[2m to exit.\x1b[0m"
            );
            let _ = writeln!(
                stderr,
                "  \x1b[2mCommands: \x1b[0m\x1b[36m/compact\x1b[0m\x1b[2m, \x1b[0m\x1b[36m/clear\x1b[0m"
            );
        }
    }
}

fn print_banner() {
    let mode = detect_color_mode();
    let fancy_underline = supports_styled_underlines();

    eprintln!();
    write_separator(&mode);
    eprintln!();
    write_art(&mode, fancy_underline);
    eprintln!();
    write_tagline(&mode);
    write_hints(&mode);
    eprintln!();
    write_separator(&mode);
    eprintln!();
}

fn read_config() -> (String, String, String) {
    let base_url = std::env::var("STRAPIN_API_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .unwrap_or_else(|_| "https://api.openai.com/v1".into());

    let api_key = std::env::var("STRAPIN_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .unwrap_or_else(|_| {
            eprintln!("Warning: No API key set. Set STRAPIN_API_KEY or OPENAI_API_KEY.");
            String::new()
        });

    let model = std::env::var("STRAPIN_MODEL").unwrap_or_else(|_| "gpt-4o".into());

    (base_url, api_key, model)
}

#[tokio::main]
async fn main() {
    print_banner();

    let (base_url, api_key, model) = read_config();

    eprintln!("  \x1b[2mendpoint:\x1b[0m \x1b[33m{base_url}\x1b[0m");
    eprintln!("  \x1b[2mmodel:\x1b[0m    \x1b[33m{model}\x1b[0m");
    eprintln!();

    let workdir = std::env::var("STRAPIN_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let client = LlmClient::new(&base_url, &api_key, &model);
    let mut agent = Agent::new(client, workdir);

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        let _ = write!(stdout, "\n\x1b[1;36m> \x1b[0m");
        let _ = stdout.flush();

        let mut input = String::new();
        match stdin.lock().read_line(&mut input) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        match input {
            "quit" | "exit" | "/quit" | "/exit" => break,
            "/compact" => {
                agent.compact(10);
                eprintln!("  \x1b[33m↻\x1b[0m \x1b[2mcontext compacted\x1b[0m");
                continue;
            }
            "/clear" => {
                let (base_url, api_key, model) = read_config();
                let workdir = std::env::var("STRAPIN_WORKDIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                let client = LlmClient::new(&base_url, &api_key, &model);
                agent = Agent::new(client, workdir);
                eprintln!("  \x1b[35m⟳\x1b[0m \x1b[2msession cleared\x1b[0m");
                continue;
            }
            _ => {}
        }

        if let Err(e) = agent.run_turn(input).await {
            eprintln!("\x1b[1;31mError: {e}\x1b[0m");
        }
    }

    eprintln!("\nBye.");
}
