//! Blocking client for Styra's JSON protocol over a Unix domain socket.

use crate::api::{
    CreateSession, Health, Request, Response, SendMessage, SessionInfo, StoredSession, Updates,
    WireRequest, WireResponse, API_VERSION,
};
use crate::journal::SessionSummary;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub fn health(&self) -> Result<Health> {
        match self.request(Request::Health)? {
            Response::Health(value) => Ok(value),
            other => unexpected("health", other),
        }
    }

    pub fn create_session(&self, request: &CreateSession) -> Result<SessionInfo> {
        match self.request(Request::CreateSession(request.clone()))? {
            Response::SessionCreated(value) => Ok(value),
            other => unexpected("session_created", other),
        }
    }

    pub fn send_message(&self, id: &str, text: &str) -> Result<()> {
        match self.request(Request::SendMessage {
            id: id.to_owned(),
            message: SendMessage {
                text: text.to_owned(),
            },
        })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    pub fn stop_session(&self, id: &str) -> Result<()> {
        match self.request(Request::StopSession { id: id.to_owned() })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    pub fn updates(&self, id: &str, after: u64) -> Result<Updates> {
        match self.request(Request::Updates {
            id: id.to_owned(),
            after,
        })? {
            Response::Updates(value) => Ok(value),
            other => unexpected("updates", other),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        match self.request(Request::ListStoredSessions)? {
            Response::StoredSessions(value) => Ok(value),
            other => unexpected("stored_sessions", other),
        }
    }

    pub fn stored_session(&self, id: &str) -> Result<StoredSession> {
        match self.request(Request::StoredSession { id: id.to_owned() })? {
            Response::StoredSession(value) => Ok(value),
            other => unexpected("stored_session", other),
        }
    }

    pub fn transcript(&self, id: &str) -> Result<String> {
        match self.request(Request::Transcript { id: id.to_owned() })? {
            Response::Transcript(value) => Ok(value.text),
            other => unexpected("transcript", other),
        }
    }

    fn request(&self, request: Request) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("connecting to Styra socket {}", self.socket.display()))?;
        let request = WireRequest {
            api_version: API_VERSION.into(),
            request,
        };
        serde_json::to_writer(&mut stream, &request).context("encoding the Styra request")?;
        stream
            .write_all(b"\n")
            .context("writing the Styra request")?;
        stream.flush().context("flushing the Styra request")?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .context("reading the Styra response")?;
        if line.is_empty() {
            bail!("Styra server closed the socket without a response");
        }
        match serde_json::from_str(&line).context("decoding the Styra response")? {
            WireResponse::Ok { response } => Ok(response),
            WireResponse::Error { error } => bail!("Styra server: {error}"),
        }
    }
}

fn unexpected<T>(expected: &str, actual: Response) -> Result<T> {
    bail!("Styra protocol error: expected {expected} response, got {actual:?}")
}
