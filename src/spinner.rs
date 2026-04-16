use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

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
                let _ = write!(stderr, "\r\x1b[90m  {frame} {msg}\x1b[0m\x1b[K");
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

    pub fn tick(&mut self, tool_name: &str) {
        self.completed += 1;
        let pct = (self.completed as f64 / self.total as f64 * 100.0) as u8;
        let filled = (self.completed * 20) / self.total;
        let bar: String = "█".repeat(filled) + &"░".repeat(20 - filled);
        eprint!("\r\x1b[90m  [{bar}] {pct:>3}% \x1b[36m{tool_name}\x1b[0m\x1b[K");
        let _ = io::stderr().flush();
    }

    pub fn finish(&self) {
        eprint!("\r\x1b[K");
        let _ = io::stderr().flush();
    }
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
        // If we get here without hanging, drop worked
    }

    #[test]
    fn tool_progress_tracks_completion() {
        let mut prog = ToolProgress::new(3);
        prog.tick("read_file");
        assert_eq!(prog.completed, 1);
        prog.tick("edit_file");
        assert_eq!(prog.completed, 2);
        prog.tick("bash");
        assert_eq!(prog.completed, 3);
    }

    #[test]
    fn tool_progress_single_tool() {
        let mut prog = ToolProgress::new(1);
        prog.tick("bash");
        assert_eq!(prog.completed, 1);
        prog.finish();
    }
}
