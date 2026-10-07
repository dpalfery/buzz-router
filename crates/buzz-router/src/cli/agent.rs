//! The agent commands `post`, `pass` and `eta` (design 12.1 and 14, R53.1–R53.3).
//!
//! Each reads `BUZZ_ROUTER_URL` and `BUZZ_ROUTER_WAKE_TOKEN`, calls the wake-token API, and
//! prints the JSON answer on stdout. Failures map to exit codes: a connection failure is
//! `Network` (2), 401 is `Auth` (3), 400 is `BadInput` (1), and every other refusal (410, 423,
//! 429, 502) is `Other` (4) with the API's error code in the message.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::{write_error, CliError};

/// The router's base URL, set by the command adapter.
const URL_VAR: &str = "BUZZ_ROUTER_URL";
/// The wake token, set by the command adapter.
const TOKEN_VAR: &str = "BUZZ_ROUTER_WAKE_TOKEN";
/// How long one call may take. A post waits for the relay's acknowledgement, with a REST
/// fallback, so this is generous.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// `post --text T | --text-file P`, where `P` is `-` for stdin. Trailing line breaks of a file
/// are dropped.
pub(super) fn post(text: Option<String>, text_file: Option<&Path>) -> Result<(), CliError> {
    let text = match (text, text_file) {
        (Some(text), _) => text,
        (None, Some(path)) => read_text(path)?.trim_end_matches(['\n', '\r']).to_owned(),
        (None, None) => return Err(CliError::bad_input("give --text or --text-file")),
    };
    agent_call("/v1/post", Some(json!({ "text": text })))
}

/// `pass`.
pub(super) fn pass() -> Result<(), CliError> {
    agent_call("/v1/pass", None)
}

/// `eta --text T`.
pub(super) fn eta(text: String) -> Result<(), CliError> {
    agent_call("/v1/eta", Some(json!({ "text": text })))
}

fn agent_call(path: &str, body: Option<Value>) -> Result<(), CliError> {
    let url = env_var(URL_VAR)?;
    let token = env_var(TOKEN_VAR)?;
    let answer = call(&url, path, &token, body, CALL_TIMEOUT).map_err(CallError::into_cli)?;
    print_json(&answer)
}

fn env_var(name: &str) -> Result<String, CliError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliError::bad_input(format!(
                "{name} is not set; agent commands run inside a wake started by the router"
            ))
        })
}

fn read_text(path: &Path) -> Result<String, CliError> {
    if path == Path::new("-") {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| CliError::bad_input(format!("cannot read standard input: {error}")))?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).map_err(|error| {
            CliError::bad_input(format!("cannot read {}: {error}", path.display()))
        })
    }
}

/// Writes `value` as one compact JSON line on stdout.
pub(super) fn print_json(value: &Value) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}").map_err(write_error)
}

/// Why a call to the router's API failed.
#[derive(Debug)]
pub(super) enum CallError {
    /// The router could not be reached: connection refused, or no answer in time.
    Unreachable(String),
    /// The router answered with an error status.
    Refused {
        /// The HTTP status.
        status: u16,
        /// The API error code, such as `halted`.
        code: String,
        /// The API's detail.
        message: String,
    },
    /// The call could not be made at all, or its answer could not be read.
    Other(String),
}

impl CallError {
    /// The CLI error for this failure (design 14).
    pub(super) fn into_cli(self) -> CliError {
        match self {
            Self::Unreachable(message) => {
                CliError::network(format!("cannot reach the router: {message}"))
            }
            Self::Refused {
                status,
                code,
                message,
            } => {
                let text = format!("{code}: {message}");
                match status {
                    401 => CliError::auth(text),
                    400 => CliError::bad_input(text),
                    _ => CliError::other(text),
                }
            }
            Self::Other(message) => CliError::other(message),
        }
    }
}

/// `POST <base_url><path>` with `Authorization: Bearer <bearer>` and an optional JSON body,
/// within `timeout`. Returns the JSON answer of a 2xx.
pub(super) fn call(
    base_url: &str,
    path: &str,
    bearer: &str,
    body: Option<Value>,
    timeout: Duration,
) -> Result<Value, CallError> {
    let bytes = request(reqwest::Method::POST, base_url, path, bearer, body, timeout)?;
    Ok(if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(
            |_| json!({ "error": "error", "message": String::from_utf8_lossy(&bytes) }),
        )
    })
}

/// `<method> <base_url><path>` with `Authorization: Bearer <bearer>` and an optional JSON body,
/// within `timeout`. Returns the raw body of a 2xx.
pub(super) fn request(
    method: reqwest::Method,
    base_url: &str,
    path: &str,
    bearer: &str,
    body: Option<Value>,
    timeout: Duration,
) -> Result<Vec<u8>, CallError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CallError::Other(format!("cannot start the async runtime: {error}")))?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| CallError::Other(format!("cannot build the HTTP client: {error}")))?;
        let url = format!("{}{path}", base_url.trim_end_matches('/'));
        let mut request = client.request(method, &url).bearer_auth(bearer);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_connect() || error.is_timeout() {
                CallError::Unreachable(error.to_string())
            } else {
                CallError::Other(format!("the request to {url} failed: {error}"))
            }
        })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            if error.is_timeout() {
                CallError::Unreachable(error.to_string())
            } else {
                CallError::Other(format!("cannot read the answer: {error}"))
            }
        })?;
        if status.is_success() {
            return Ok(bytes.to_vec());
        }
        let answer: Value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(
                |_| json!({ "error": "error", "message": String::from_utf8_lossy(&bytes) }),
            )
        };
        let field = |name: &str| {
            answer
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Err(CallError::Refused {
            status: status.as_u16(),
            code: Some(field("error"))
                .filter(|code| !code.is_empty())
                .unwrap_or_else(|| status.as_u16().to_string()),
            message: field("message"),
        })
    })
}
