use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

// ANSI color codes
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const BLUE: &str = "\x1b[34m";

const GRADIENT: &[&str] = &[
    "\x1b[36m", // cyan
    "\x1b[96m", // bright cyan
    "\x1b[34m", // blue
    "\x1b[94m", // bright blue
    "\x1b[35m", // magenta
    "\x1b[95m", // bright magenta
    "\x1b[36m", // cyan
];

const BRAILLE: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DOTS: &[&str] = &["   ", ".  ", ".. ", "...", " ..", "  .", "   "];
const BOUNCE: &[&str] = &[
    "●∙∙∙∙",
    "∙●∙∙∙",
    "∙∙●∙∙",
    "∙∙∙●∙",
    "∙∙∙∙●",
    "∙∙∙●∙",
    "∙∙●∙∙",
    "∙●∙∙∙",
];

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum Style {
    Braille,
    Dots,
    Bounce,
}

impl Style {
    fn frames(self) -> &'static [&'static str] {
        match self {
            Style::Braille => BRAILLE,
            Style::Dots => DOTS,
            Style::Bounce => BOUNCE,
        }
    }

    fn interval_ms(self) -> u64 {
        match self {
            Style::Braille => 80,
            Style::Dots => 300,
            Style::Bounce => 120,
        }
    }
}

pub struct Spinner {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn start(message: &str, style: Style) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let msg = message.to_string();
        let frames = style.frames();
        let interval = style.interval_ms();

        let handle = tokio::spawn(async move {
            let mut stderr = io::stderr();
            let mut i = 0;
            while running_clone.load(Ordering::Relaxed) {
                let frame = frames[i % frames.len()];
                let color = GRADIENT[i % GRADIENT.len()];
                let _ = write!(
                    stderr,
                    "\r  {color}{BOLD}{frame}{RESET} {DIM}{msg}{RESET}\x1b[K"
                );
                let _ = stderr.flush();
                i += 1;
                time::sleep(Duration::from_millis(interval)).await;
            }
            let _ = write!(stderr, "\r\x1b[K");
            let _ = stderr.flush();
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    /// Spinner variant that shows a tool name in cyan plus grey commentary.
    pub fn start_tool(name: &str, commentary: &str, style: Style) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let name = name.to_string();
        let commentary = commentary.to_string();
        let frames = style.frames();
        let interval = style.interval_ms();

        let handle = tokio::spawn(async move {
            let mut stderr = io::stderr();
            let mut i = 0;
            while running_clone.load(Ordering::Relaxed) {
                let frame = frames[i % frames.len()];
                let color = GRADIENT[i % GRADIENT.len()];
                if commentary.is_empty() {
                    let _ = write!(
                        stderr,
                        "\r  {color}{BOLD}{frame}{RESET} {CYAN}{name}{RESET}\x1b[K"
                    );
                } else {
                    let _ = write!(
                        stderr,
                        "\r  {color}{BOLD}{frame}{RESET} {CYAN}{name}{RESET} {DIM}{commentary}{RESET}\x1b[K"
                    );
                }
                let _ = stderr.flush();
                i += 1;
                time::sleep(Duration::from_millis(interval)).await;
            }
            let _ = write!(stderr, "\r\x1b[K");
            let _ = stderr.flush();
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    pub async fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
    }

    #[cfg(test)]
    pub fn stop_sync(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Progress bar for multi-tool execution.
pub struct ToolProgress {
    total: usize,
    completed: usize,
}

impl ToolProgress {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            completed: 0,
        }
    }

    pub fn tick(&mut self, tool_name: &str, commentary: &str) {
        self.completed += 1;
        let pct = (self.completed as f64 / self.total as f64 * 100.0) as u8;
        let filled = (self.completed * 20) / self.total;
        let empty = 20 - filled;

        let bar_color = if pct < 33 {
            CYAN
        } else if pct < 66 {
            BLUE
        } else {
            GREEN
        };

        let filled_bar = "█".repeat(filled);
        let empty_bar = "░".repeat(empty);
        if commentary.is_empty() {
            eprint!(
                "\r  {bar_color}{filled_bar}{DIM}{empty_bar}{RESET} {YELLOW}{pct:>3}%{RESET} {MAGENTA}{tool_name}{RESET}\x1b[K"
            );
        } else {
            eprint!(
                "\r  {bar_color}{filled_bar}{DIM}{empty_bar}{RESET} {YELLOW}{pct:>3}%{RESET} {MAGENTA}{tool_name}{RESET} {DIM}{commentary}{RESET}\x1b[K"
            );
        }
        let _ = io::stderr().flush();
    }

    pub fn finish(&self) {
        eprint!("\r\x1b[K");
        let _ = io::stderr().flush();
    }
}

/// Print a colored tool completion message.
pub fn print_tool_done(name: &str, commentary: &str, detail: &str) {
    if commentary.is_empty() {
        eprintln!("  {GREEN}{BOLD}✓{RESET} {CYAN}{name}{RESET} {DIM}({detail}){RESET}");
    } else {
        eprintln!(
            "  {GREEN}{BOLD}✓{RESET} {CYAN}{name}{RESET} {DIM}{commentary}{RESET} {DIM}({detail}){RESET}"
        );
    }
}

/// Print a colored multi-tool completion message.
pub fn print_tools_done(count: usize) {
    eprintln!("  {GREEN}{BOLD}✓{RESET} {CYAN}{count} tools{RESET} {DIM}completed{RESET}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_has_10_frames() {
        assert_eq!(Style::Braille.frames().len(), 10);
    }

    #[test]
    fn dots_has_7_frames() {
        assert_eq!(Style::Dots.frames().len(), 7);
    }

    #[test]
    fn bounce_has_8_frames() {
        assert_eq!(Style::Bounce.frames().len(), 8);
    }

    #[test]
    fn intervals_are_reasonable() {
        assert!(Style::Braille.interval_ms() > 0 && Style::Braille.interval_ms() < 200);
        assert!(Style::Dots.interval_ms() > 0 && Style::Dots.interval_ms() < 500);
        assert!(Style::Bounce.interval_ms() > 0 && Style::Bounce.interval_ms() < 200);
    }

    #[test]
    fn gradient_has_entries() {
        assert!(!GRADIENT.is_empty());
        for g in GRADIENT {
            assert!(g.starts_with("\x1b["));
        }
    }

    #[tokio::test]
    async fn spinner_starts_and_stops() {
        let spinner = Spinner::start("testing", Style::Braille);
        tokio::time::sleep(Duration::from_millis(200)).await;
        spinner.stop().await;
    }

    #[tokio::test]
    async fn spinner_stop_sync_sets_flag() {
        let mut spinner = Spinner::start("testing", Style::Dots);
        tokio::time::sleep(Duration::from_millis(100)).await;
        spinner.stop_sync();
        assert!(!spinner.running.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn spinner_drop_stops_animation() {
        {
            let _spinner = Spinner::start("drop test", Style::Bounce);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[test]
    fn tool_progress_tracks_completion() {
        let mut prog = ToolProgress::new(3);
        prog.tick("read_file", "foo.rs");
        assert_eq!(prog.completed, 1);
        prog.tick("edit_file", "bar.rs");
        assert_eq!(prog.completed, 2);
        prog.tick("bash", "");
        assert_eq!(prog.completed, 3);
    }

    #[test]
    fn tool_progress_single_tool() {
        let mut prog = ToolProgress::new(1);
        prog.tick("bash", "ls -la");
        assert_eq!(prog.completed, 1);
        prog.finish();
    }

    #[test]
    fn progress_bar_color_changes_with_pct() {
        let mut prog = ToolProgress::new(10);
        prog.tick("t1", "");
        // 10% — should be cyan range
        assert_eq!(prog.completed, 1);
        for i in 2..=6 {
            prog.tick(&format!("t{i}"), "");
        }
        // 60% — should be blue range
        assert_eq!(prog.completed, 6);
        for i in 7..=10 {
            prog.tick(&format!("t{i}"), "");
        }
        // 100% — should be green range
        assert_eq!(prog.completed, 10);
    }

    #[tokio::test]
    async fn spinner_start_tool_starts_and_stops() {
        let spinner = Spinner::start_tool("read_file", "src/main.rs", Style::Bounce);
        tokio::time::sleep(Duration::from_millis(100)).await;
        spinner.stop().await;
    }

    #[tokio::test]
    async fn spinner_start_tool_handles_empty_commentary() {
        let spinner = Spinner::start_tool("bash", "", Style::Dots);
        tokio::time::sleep(Duration::from_millis(50)).await;
        spinner.stop().await;
    }
}
