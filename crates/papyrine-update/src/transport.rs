//! HTTP transport. The only thing that touches the network.
//!
//! A request is just a URL: GET, no query, no cookies, and a single generic
//! `User-Agent`. [`Transport`] is a trait so tests can record requests.

use crate::{Error, Result};
use std::io::Read;
use std::time::Duration;

pub const USER_AGENT: &str = "Papyrine";

/// A GET request. There is deliberately no field for custom headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
}

pub struct Response {
    pub status: u16,
    pub body: Box<dyn Read + Send>,
}

pub trait Transport {
    fn get(&self, req: &Request) -> Result<Response>;
}

/// ureq + rustls transport (HTTPS only).
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        Self::build(true)
    }

    /// Plain HTTP, for wire-level tests against a loopback server only.
    #[doc(hidden)]
    pub fn plain_http_for_tests() -> Self {
        Self::build(false)
    }

    fn build(https_only: bool) -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(https_only)
            .user_agent(USER_AGENT)
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(5)
            // Redirects on GitHub go to another host: never forward anything.
            .max_redirects_will_error(true)
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for UreqTransport {
    fn get(&self, req: &Request) -> Result<Response> {
        let resp = self
            .agent
            .get(&req.url)
            .call()
            .map_err(|e| Error::Io(e.to_string()))?;
        let status = resp.status().as_u16();
        let reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
        Ok(Response {
            status,
            body: Box::new(reader),
        })
    }
}

/// Read at most `max` bytes; error if the body is larger.
pub fn read_limited(mut body: impl Read, max: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    (&mut body)
        .take(max as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| Error::Io(e.to_string()))?;
    if buf.len() > max {
        return Err(Error::Malformed("response too large".into()));
    }
    Ok(buf)
}
