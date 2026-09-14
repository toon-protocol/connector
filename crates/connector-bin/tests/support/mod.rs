//! Shared process-spawning support for `connector-bin`'s black-box tests:
//! drives the real, compiled `connector`/`stub-app` binaries rather than
//! having each test binary reimplement the spawning.
//!
//! Not every consumer uses every helper (`refuses_to_start.rs` still has
//! its own smaller variant for its narrower needs), so this module is
//! allowed dead code per test binary rather than forcing every caller to
//! use every function.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use chrono::{Duration as ChronoDuration, Utc};
use connector_domain::{EnvelopeRequest, Prepare};
use connector_signer::giftwrap::seal_request;
use connector_signer::{LocalSigner, PublicKeyBytes, Signer};

/// A spawned child process, killed and reaped on drop -- so a test that
/// panics midway still leaves no orphaned process behind.
pub struct Process {
    child: Child,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn write_config(text: &str) -> tempfile::NamedTempFile {
    let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
    write!(config_file, "{text}").expect("write config file");
    config_file
}

pub fn write_raw_key_file(seed: u8) -> tempfile::NamedTempFile {
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file
        .write_all(&[seed; 32])
        .expect("write raw 32-byte key");
    key_file
}

/// The identity `write_raw_key_file(seed)` produces, reconstructed the same
/// way `connector-cli::runtime::build` derives a signer from raw key-file
/// bytes -- so a test can seal to a real connector process's actual
/// identity without needing to ask it over the wire.
pub fn identity_from_key_seed(seed: u8) -> PublicKeyBytes {
    LocalSigner::from_secret_bytes("test-identity", [seed; 32])
        .expect("valid key seed")
        .public_key()
        .expect("public key")
}

/// ADR 0018/issue #524: a terminated route's `Prepare.data` is a gift wrap
/// sealed to the terminating connector's identity -- `receiver_public`,
/// derived the same way `runtime::build()` derives it in the real binary
/// (from the `[signer] key_file` raw bytes) -- around a minimal `POST /`
/// envelope carrying `body`. Returns the wire bytes for `Prepare.data` and
/// the shared secret the wrap carries, to open the sealed `Fulfill`/`Reject`
/// this produces or to compute the fulfilment ADR 0019/issue #525 derives
/// from it.
pub fn sealed_prepare_data(body: &[u8], receiver_public: &PublicKeyBytes) -> (Vec<u8>, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: body.to_vec(),
    }
    .encode();
    seal_request(&plaintext, receiver_public).expect("seal")
}

/// A `Prepare` carrying `data` -- typically already sealed with a secret the
/// termination will derive its fulfilment from (ADR 0019, issue #525) via
/// [`sealed_prepare_data`].
pub fn sample_prepare(destination: &str, data: Vec<u8>) -> Prepare {
    Prepare {
        amount: 0,
        expires_at: Utc::now() + ChronoDuration::minutes(5),
        greeting: false,
        destination: destination.to_string(),
        data,
    }
}

/// Parse the `addr` field out of one of the connector binary's structured
/// JSON tracing log lines.
pub fn parse_json_log_addr(line: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(line).expect("JSON log line");
    parsed["fields"]["addr"]
        .as_str()
        .expect("addr field")
        .to_string()
}

pub struct StubApp {
    _process: Process,
    pub addr: String,
}

pub fn spawn_stub_app() -> StubApp {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stub-app"))
        .arg("127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn stub-app");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let process = Process { child };

    let mut line = String::new();
    let addr = loop {
        line.clear();
        let read = stdout.read_line(&mut line).expect("read stdout");
        assert!(read > 0, "stub-app exited before printing its address");
        if let Some(addr) = line.trim().strip_prefix("stub-app listening ") {
            break addr.to_string();
        }
    };

    StubApp {
        addr,
        _process: process,
    }
}

pub struct ConnectorProcess {
    _process: Process,
    pub client_edge_addr: String,
}

pub fn spawn_connector(config_path: &std::path::Path) -> ConnectorProcess {
    let mut child = Command::new(env!("CARGO_BIN_EXE_connector"))
        .arg(config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn connector");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));

    // Drained from its own thread, from the moment the process starts, for
    // two reasons.
    //
    // The first is diagnostic: a node that exits before it listens exits
    // *for a reason* -- ADR 0009 makes every startup refusal a named message
    // here -- and a failure that reports only "it exited" costs a bisect to
    // recover what one line already said.
    //
    // The second is that an undrained pipe is a **deadlock**, not merely a
    // lost message. A pipe holds ~64 KiB; once it fills, the connector's own
    // `tracing` writer blocks inside whatever task is logging -- which, on
    // the packet path, is the task answering the request the test is waiting
    // on. The symptom is a `hyper::Error(IncompleteMessage)` at the test,
    // arbitrarily far from the cause, and it appears only once a test drives
    // enough traffic to fill the buffer.
    let mut stderr = child.stderr.take().expect("piped stderr");
    let startup_error = Arc::new(Mutex::new(String::new()));
    {
        let startup_error = startup_error.clone();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            startup_error
                .lock()
                .expect("stderr buffer lock poisoned")
                .push_str(&text);
        });
    }

    let process = Process { child };

    // One listener since ADR 0027 / issue #679 deleted the raw-TCP peer
    // wire's second one: read until "connector listening" is seen.
    let mut line = String::new();
    let client_edge_addr = loop {
        line.clear();
        let read = stdout.read_line(&mut line).expect("read stdout");
        if read == 0 {
            // The draining thread may still be mid-read; give it the moment
            // it needs so the panic carries the refusal rather than "".
            std::thread::sleep(std::time::Duration::from_millis(200));
            let reason = startup_error
                .lock()
                .expect("stderr buffer lock poisoned")
                .clone();
            panic!(
                "connector exited before logging a listen address: {}",
                reason.trim()
            );
        }
        if line.contains("connector listening") {
            break parse_json_log_addr(&line);
        }
    };

    // And keep draining stdout for the process's whole life, for the same
    // deadlock reason: the listen line is the *first* of many.
    std::thread::spawn(move || {
        let mut sink = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut sink);
    });

    ConnectorProcess {
        _process: process,
        client_edge_addr,
    }
}
