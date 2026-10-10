//! `marrow serve`: the local dashboard (PRD §7.1), for the repository in the current directory
//! (docs/decisions/0013-serve.md).
//!
//! A small HTTP server on 127.0.0.1 hands out one page and the same numbers `report`, `status`,
//! `explain` and `doctor` print. It listens on the loopback address only, answers only requests
//! addressed to that address by name, and tells the browser the page may talk to nothing but this
//! server, so nothing the dashboard shows can leave the machine through it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use marrow_store::Store;
use serde_json::{json, Value};

use crate::capture::repository_root;
use crate::report::{self, By};
use crate::{doctor, explain, status};

/// The dashboard. It is built into the binary, so serving it needs no files and no network.
const PAGE: &str = include_str!("../../../../web/index.html");

/// The page may run its own inline script and style and fetch from this server, and nothing
/// else: no other origin, no frames, no forms.
const POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                      connect-src 'self'; img-src data:; base-uri 'none'; form-action 'none'; \
                      frame-ancestors 'none'";

/// A request's head is a few hundred bytes; anything far larger isn't a browser asking for a page.
const MAX_HEAD: usize = 16 * 1024;
/// More resamples than this takes long enough to look like a hang.
const MAX_RESAMPLES: usize = 2_000;

pub fn serve(port: u16) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    if !root.join(".marrow").join("db.sqlite").exists() {
        return Err(
            "marrow has no records for this repository yet; run marrow init or marrow backfill"
                .to_owned(),
        );
    }
    // Every answer is worked out from the repository's root, so a line the page asks about is
    // named the way git names it, wherever the server was started from.
    std::env::set_current_dir(&root)
        .map_err(|error| format!("can't move to {}: {error}", root.display()))?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("can't listen on 127.0.0.1:{port}: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("{error}"))?
        .port();
    println!(
        "marrow serve: http://127.0.0.1:{port}/ (only this machine can reach it; Ctrl+C stops it)"
    );
    let _ = std::io::stdout().flush();
    for stream in listener.incoming() {
        // A connection that failed to open is the browser's problem, not a reason to stop.
        let Ok(stream) = stream else { continue };
        // A report can take a moment; one slow answer shouldn't hold up the rest of the page.
        std::thread::spawn(move || answer(stream, port));
    }
    Ok(())
}

fn answer(mut stream: TcpStream, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let response = match read_head(&mut stream) {
        Some(head) => match Request::parse(&head) {
            Some(request) => respond(&request, port),
            None => Response::error(400, "that isn't an HTTP request marrow understands"),
        },
        None => Response::error(400, "the request didn't arrive whole"),
    };
    let _ = stream.write_all(&response.bytes());
}

/// Everything up to the blank line that ends a request's headers. A GET has no body, so that is
/// the whole request.
fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 || head.len() + read > MAX_HEAD {
            return None;
        }
        head.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(head).ok()
}

pub struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    host: Option<String>,
}

impl Request {
    pub fn parse(head: &str) -> Option<Request> {
        let mut lines = head.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let method = first.next()?.to_owned();
        let target = first.next()?;
        if !first.next()?.starts_with("HTTP/1.") {
            return None;
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let host = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim().to_ascii_lowercase());
        Some(Request {
            method,
            path: decode(path),
            query: parse_query(query),
            host,
        })
    }
}

pub struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: &Value) -> Response {
        Response {
            status,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
        }
    }

    fn error(status: u16, message: &str) -> Response {
        Response::json(status, &json!({"error": message}))
    }

    pub fn bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            _ => "Internal Server Error",
        };
        let allow = if self.status == 405 {
            "Allow: GET\r\n"
        } else {
            ""
        };
        let mut bytes = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
             Referrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n\
             Content-Security-Policy: {POLICY}\r\n{allow}Connection: close\r\n\r\n",
            self.status,
            self.content_type,
            self.body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

pub fn respond(request: &Request, port: u16) -> Response {
    // A page on another site can point a name it controls at 127.0.0.1 and have the browser
    // send it here (DNS rebinding). Such a request still carries that name, so only requests
    // addressed to this machine by its own names are answered.
    let addressed_here = request.host.as_deref().is_some_and(|host| {
        [format!("127.0.0.1:{port}"), format!("localhost:{port}")].contains(&host.to_owned())
    });
    if !addressed_here {
        return Response::error(
            403,
            &format!("open the dashboard as http://127.0.0.1:{port}/ or http://localhost:{port}/"),
        );
    }
    if request.method != "GET" {
        return Response::error(405, "the dashboard only reads");
    }
    let query = |name: &str| request.query.get(name).map(String::as_str);
    match request.path.as_str() {
        "/" | "/index.html" => Response {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: PAGE.as_bytes().to_vec(),
        },
        "/api/overview" => overview(),
        "/api/report" => match report_options(query) {
            Ok(options) => match report::compute(&options) {
                Ok(computed) => Response::json(200, &report::to_json(&computed)),
                Err(message) => Response::error(500, &message),
            },
            Err(message) => Response::error(400, &message),
        },
        "/api/status" => match status::status(query("session").filter(|id| !id.is_empty())) {
            Ok(text) => Response::json(200, &json!({"text": text})),
            Err(message) => Response::error(400, &message),
        },
        "/api/explain" => match query("target").filter(|target| !target.is_empty()) {
            Some(target) => match explain::explain(target) {
                Ok(text) => Response::json(200, &json!({"text": text})),
                Err(message) => Response::error(400, &message),
            },
            None => Response::error(400, "name a line as <file>:<line>"),
        },
        "/api/doctor" => match doctor::doctor() {
            Ok(checkup) => Response::json(
                200,
                &json!({"text": checkup.text, "healthy": checkup.healthy}),
            ),
            Err(message) => Response::error(500, &message),
        },
        _ => Response::error(404, "nothing here"),
    }
}

/// The repository's name, the engine's version, and the agent sessions recorded, newest first.
fn overview() -> Response {
    let found = (|| -> Result<Value, String> {
        let root = std::env::current_dir().map_err(|error| format!("{error}"))?;
        let store = Store::open(&root.join(".marrow").join("db.sqlite"))
            .map_err(|error| format!("{error}"))?;
        let sessions: Vec<Value> = store
            .sessions()
            .map_err(|error| format!("{error}"))?
            .into_iter()
            .map(|session| {
                json!({
                    "id": session.session_id,
                    "model": session.model,
                    "started_at": session.started_at,
                })
            })
            .collect();
        Ok(json!({
            "repository": root.file_name().map(|name| name.to_string_lossy().into_owned()),
            "version": env!("CARGO_PKG_VERSION"),
            "sessions": sessions,
        }))
    })();
    match found {
        Ok(value) => Response::json(200, &value),
        Err(message) => Response::error(500, &message),
    }
}

/// `report`'s options from the query, with `report`'s defaults and limits.
fn report_options<'a>(query: impl Fn(&str) -> Option<&'a str>) -> Result<report::Options, String> {
    let by = match query("by") {
        Some(text) => By::parse(text).ok_or("by is origin, role, or file")?,
        None => By::Origin,
    };
    let threshold = match query("threshold") {
        Some(text) => text
            .parse::<f64>()
            .ok()
            .filter(|threshold| *threshold > 0.0 && *threshold < 1.0)
            .ok_or("threshold must be between 0 and 1")?,
        None => 0.5,
    };
    let resamples = match query("resamples") {
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|resamples| *resamples <= MAX_RESAMPLES)
            .ok_or_else(|| format!("resamples must be a whole number up to {MAX_RESAMPLES}"))?,
        None => 200,
    };
    let seed = match query("seed") {
        Some(text) => text
            .parse::<u64>()
            .map_err(|_| "seed must be a whole number")?,
        None => 1,
    };
    Ok(report::Options {
        by,
        threshold,
        resamples,
        seed,
    })
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

/// Undoes URL encoding: `%XX` is a byte, and in a query `+` is a space.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|&byte| (byte as char).to_digit(16));
        match bytes[index] {
            b'%' => match (hex(index + 1), hex(index + 2)) {
                (Some(high), Some(low)) => {
                    decoded.push((high * 16 + low) as u8);
                    index += 3;
                    continue;
                }
                _ => decoded.push(b'%'),
            },
            b'+' => decoded.push(b' '),
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{decode, respond, Request};

    fn get(target: &str, host: &str) -> Request {
        Request::parse(&format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n")).unwrap()
    }

    fn head(request: &Request) -> (u16, String) {
        let response = respond(request, 4242);
        let bytes = response.bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (response.status, text)
    }

    #[test]
    fn a_query_is_decoded() {
        let request = get(
            "/api/explain?target=src%2Fmain.rs%3A12&x=a+b",
            "127.0.0.1:4242",
        );
        assert_eq!(request.query["target"], "src/main.rs:12");
        assert_eq!(request.query["x"], "a b");
        assert_eq!(decode("100%"), "100%", "a stray percent stays");
        assert_eq!(decode("%e2%80%a6"), "…");
    }

    #[test]
    fn the_page_comes_with_a_policy_that_keeps_it_talking_only_to_this_server() {
        let (status, text) = head(&get("/", "127.0.0.1:4242"));
        assert_eq!(status, 200);
        assert!(text.contains("Content-Security-Policy: default-src 'none';"));
        assert!(text.contains("connect-src 'self'"));
        assert!(text.contains("<!doctype html>"));
    }

    #[test]
    fn a_request_addressed_to_another_name_is_refused() {
        // What a DNS-rebinding page's requests look like when they arrive.
        assert_eq!(head(&get("/", "attacker.example:4242")).0, 403);
        assert_eq!(head(&get("/", "127.0.0.1:9999")).0, 403, "another port");
        assert_eq!(head(&get("/", "localhost:4242")).0, 200);
        let no_host = Request::parse("GET / HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(head(&no_host).0, 403);
    }

    #[test]
    fn only_reading_is_allowed() {
        let post =
            Request::parse("POST /api/report HTTP/1.1\r\nHost: 127.0.0.1:4242\r\n\r\n").unwrap();
        let (status, text) = head(&post);
        assert_eq!(status, 405);
        assert!(text.contains("Allow: GET"));
    }

    #[test]
    fn bad_options_and_unknown_paths_are_answered_not_crashed_on() {
        assert_eq!(head(&get("/nothing", "127.0.0.1:4242")).0, 404);
        for target in [
            "/api/report?by=colour",
            "/api/report?threshold=1.5",
            "/api/report?resamples=999999",
            "/api/report?seed=-1",
            "/api/explain",
        ] {
            assert_eq!(head(&get(target, "127.0.0.1:4242")).0, 400, "{target}");
        }
        assert!(Request::parse("not http at all\r\n\r\n").is_none());
    }
}
