//! `buzz-router-test-agent`: a scripted agent process for buzz-router's tests (design 16.3).
//!
//! Modes:
//!
//! - `echo --delay <secs>`: reads the prompt (from `BUZZ_ROUTER_PROMPT_FILE` when set, else
//!   stdin), waits, and prints `echo: <prompt>`.
//! - `print-env`: prints `NAME=value` for every `BUZZ_*` and `AGENT_*` variable, sorted, then
//!   `CWD=<working directory>`.
//! - `write-stdout --bytes N`: writes N bytes to stdout.
//! - `exit --code N [--stdout T]`: prints T, then exits with N.
//! - `stderr --text T`: writes T to stderr.
//! - `spawn-grandchild <pidfile>`: starts a sleeping child copy, writes both pids (its own, then
//!   the child's, one per line) to the file, and sleeps.
//! - `api-post --text T`: posts T through the router API with the wake token.
//! - `api-pass-then-sleep`: passes through the router API, then sleeps.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

/// How long the sleeping modes sleep: longer than any test waits.
const LONG_SLEEP: Duration = Duration::from_secs(600);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("buzz-router-test-agent: {error}");
            ExitCode::from(100)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let mode = args.first().map(String::as_str).unwrap_or_default();
    let rest = args.get(1..).unwrap_or_default();
    match mode {
        "echo" => {
            let delay: u64 = flag(rest, "--delay").unwrap_or("0").parse().map_err(text)?;
            let prompt = match std::env::var("BUZZ_ROUTER_PROMPT_FILE") {
                Ok(path) => std::fs::read_to_string(path).map_err(text)?,
                Err(_) => {
                    let mut prompt = String::new();
                    std::io::stdin().read_to_string(&mut prompt).map_err(text)?;
                    prompt
                }
            };
            std::thread::sleep(Duration::from_secs(delay));
            print!("echo: {prompt}");
        }
        "print-env" => {
            let mut vars: Vec<(String, String)> = std::env::vars()
                .filter(|(name, _)| name.starts_with("BUZZ_") || name.starts_with("AGENT_"))
                .collect();
            vars.sort();
            for (name, value) in vars {
                println!("{name}={value}");
            }
            let cwd = std::env::current_dir().map_err(text)?;
            println!("CWD={}", cwd.display());
        }
        "write-stdout" => {
            let bytes: usize = flag(rest, "--bytes").unwrap_or("0").parse().map_err(text)?;
            let mut out = std::io::stdout().lock();
            let chunk = [b'x'; 8_192];
            let mut left = bytes;
            while left > 0 {
                let n = left.min(chunk.len());
                out.write_all(&chunk[..n]).map_err(text)?;
                left -= n;
            }
            out.flush().map_err(text)?;
        }
        "exit" => {
            let code: u8 = flag(rest, "--code").unwrap_or("0").parse().map_err(text)?;
            if let Some(stdout) = flag(rest, "--stdout") {
                print!("{stdout}");
            }
            return Ok(ExitCode::from(code));
        }
        "stderr" => eprintln!("{}", flag(rest, "--text").unwrap_or_default()),
        "spawn-grandchild" => {
            let pidfile = rest.first().ok_or("spawn-grandchild needs a pid file")?;
            let exe = std::env::current_exe().map_err(text)?;
            let child = std::process::Command::new(exe)
                .arg("sleep")
                .spawn()
                .map_err(text)?;
            let pids = format!("{}\n{}\n", std::process::id(), child.id());
            std::fs::write(pidfile, pids).map_err(text)?;
            std::thread::sleep(LONG_SLEEP);
        }
        "sleep" => std::thread::sleep(LONG_SLEEP),
        "api-post" => {
            let body = format!(
                "{{\"text\":{}}}",
                json_string(flag(rest, "--text").unwrap_or_default())
            );
            api("/v1/post", &body)?;
        }
        "api-pass-then-sleep" => {
            api("/v1/pass", "{}")?;
            std::thread::sleep(LONG_SLEEP);
        }
        other => return Err(format!("unknown mode {other:?}")),
    }
    Ok(ExitCode::SUCCESS)
}

/// The value after `name` in `args`.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// `text` as a JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// POSTs `body` to `path` on `BUZZ_ROUTER_URL` with the wake token, over plain HTTP/1.1, and
/// fails unless the answer is 200.
fn api(path: &str, body: &str) -> Result<(), String> {
    let base = std::env::var("BUZZ_ROUTER_URL").map_err(text)?;
    let token = std::env::var("BUZZ_ROUTER_WAKE_TOKEN").map_err(text)?;
    let host = base
        .strip_prefix("http://")
        .ok_or("BUZZ_ROUTER_URL is not http://")?
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(host).map_err(text)?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(text)?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(text)?;
    if response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!(
            "{path} answered {}",
            response.lines().next().unwrap_or_default()
        ))
    }
}
