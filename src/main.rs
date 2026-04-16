mod agent;
mod client;
mod spinner;
mod tools;
mod types;

use agent::Agent;
use client::LlmClient;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

fn print_banner() {
    eprintln!(
        r#"
  _____ _______ _____            _____    _____ _   _
 / ____|__   __|  __ \     /\   |  __ \  |_   _| \ | |
| (___    | |  | |__) |   /  \  | |__) |   | | |  \| |
 \___ \   | |  |  _  /   / /\ \ |  ___/    | | | . ` |
 ____) |  | |  | | \ \  / ____ \| |       _| |_| |\  |
|_____/   |_|  |_|  \_\/_/    \_\_|      |_____|_| \_|

  Fast coding agent harness — any OpenAI-compatible endpoint
  Type your request, or 'quit' to exit.
  Commands: /compact, /clear
"#
    );
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

    eprintln!("  endpoint: {base_url}");
    eprintln!("  model:    {model}");
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
                eprintln!("\x1b[90m[compacted context]\x1b[0m");
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
                eprintln!("\x1b[90m[session cleared]\x1b[0m");
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
