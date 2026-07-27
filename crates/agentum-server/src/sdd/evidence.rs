//! Typed, redacted browser-verification evidence.
//!
//! Browser drivers may contain page text, cookies, query parameters, and
//! console output. None of that raw material crosses this contract. Agentum
//! records bounded metadata and content-addressed blob references; blobs live
//! in Agentum-owned storage outside the customer repository.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

pub const BROWSER_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const MAX_CAPTURE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CAPTURES: usize = 32;
const MAX_ASSERTIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserEvidence {
    pub schema_version: u32,
    pub evidence_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub check_id: String,
    pub spec_revision: i64,
    pub captured_at: String,
    pub workspace_fingerprint: String,
    pub target: BrowserTarget,
    pub browser: BrowserRuntime,
    pub captures: Vec<BrowserCaptureRef>,
    pub assertions: Vec<BrowserAssertion>,
    pub console: BrowserConsoleSummary,
    pub network: BrowserNetworkSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserTarget {
    /// Scheme + authority only. Credentials, paths, query, and fragments are
    /// rejected. `path` is carried separately after the driver redacts it.
    pub origin: String,
    pub path: String,
    pub path_redacted: bool,
    pub query_redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserRuntime {
    pub name: String,
    pub version: String,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub device_scale_milli: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserCaptureKind {
    Screenshot,
    DomSnapshot,
    AccessibilityTree,
    Trace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserCaptureRef {
    pub kind: BrowserCaptureKind,
    pub sha256: String,
    pub byte_length: u64,
    pub media_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAssertionStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserAssertion {
    pub id: String,
    pub status: BrowserAssertionStatus,
    pub acceptance_criteria: Vec<String>,
    /// Hashes of capture blobs that support this assertion. Human-readable
    /// failure detail belongs in the independently authored review, not here.
    pub evidence_sha256: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserConsoleSummary {
    pub coverage: BrowserDiagnosticCoverage,
    pub errors: u32,
    pub warnings: u32,
    /// Hash of the redacted, bounded driver transcript. Raw console messages
    /// are deliberately not part of this DTO.
    pub transcript_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserNetworkSummary {
    pub coverage: BrowserDiagnosticCoverage,
    pub requests: u32,
    pub failed_requests: u32,
    /// Hash of the redacted request summary; URLs and headers are not stored.
    pub transcript_sha256: String,
}

/// Makes diagnostic provenance explicit. Agentum never relabels the CDP
/// driver's process-global diagnostic buffers as attempt-owned evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDiagnosticCoverage {
    None,
    MainDocument,
    FullContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvidenceBlob {
    pub sha256: String,
    pub byte_length: i64,
    pub media_type: String,
    pub storage_relative_path: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BrowserEvidenceError {
    #[error("unsupported browser evidence schema")]
    Schema,
    #[error("browser evidence identity is invalid")]
    Identity,
    #[error("browser evidence timestamp is invalid")]
    Timestamp,
    #[error("browser target is not safely redacted")]
    UnsafeTarget,
    #[error("browser runtime metadata is invalid")]
    Runtime,
    #[error("browser capture metadata is invalid")]
    Capture,
    #[error("browser assertion metadata is invalid")]
    Assertion,
    #[error("browser console or network summary is invalid")]
    Summary,
    #[error("browser capture bytes do not match their immutable reference")]
    CaptureMismatch,
    #[error("browser evidence could not be encoded: {0}")]
    Encoding(String),
}

impl BrowserEvidence {
    pub fn validate(&self) -> Result<(), BrowserEvidenceError> {
        if self.schema_version != BROWSER_EVIDENCE_SCHEMA_VERSION {
            return Err(BrowserEvidenceError::Schema);
        }
        if Uuid::parse_str(&self.evidence_id).is_err()
            || Uuid::parse_str(&self.run_id).is_err()
            || Uuid::parse_str(&self.attempt_id).is_err()
            || self.check_id.trim().is_empty()
            || self.check_id.len() > 64
            || self.spec_revision < 1
            || !valid_sha256(&self.workspace_fingerprint)
        {
            return Err(BrowserEvidenceError::Identity);
        }
        if OffsetDateTime::parse(&self.captured_at, &Rfc3339).is_err() {
            return Err(BrowserEvidenceError::Timestamp);
        }
        validate_target(&self.target)?;
        validate_runtime(&self.browser)?;
        if self.captures.is_empty() || self.captures.len() > MAX_CAPTURES {
            return Err(BrowserEvidenceError::Capture);
        }
        let mut capture_hashes = HashSet::new();
        for capture in &self.captures {
            validate_capture(capture)?;
            if !capture_hashes.insert(capture.sha256.as_str()) {
                return Err(BrowserEvidenceError::Capture);
            }
        }
        if self.assertions.is_empty() || self.assertions.len() > MAX_ASSERTIONS {
            return Err(BrowserEvidenceError::Assertion);
        }
        let mut assertion_ids = HashSet::new();
        for assertion in &self.assertions {
            if !valid_assertion_id(&assertion.id)
                || !assertion_ids.insert(assertion.id.as_str())
                || assertion.acceptance_criteria.is_empty()
                || assertion
                    .acceptance_criteria
                    .iter()
                    .any(|criterion| !valid_acceptance_criterion(criterion))
                || assertion.evidence_sha256.is_empty()
                || assertion
                    .evidence_sha256
                    .iter()
                    .any(|hash| !capture_hashes.contains(hash.as_str()))
            {
                return Err(BrowserEvidenceError::Assertion);
            }
        }
        if self.network.failed_requests > self.network.requests
            || (self.console.coverage == BrowserDiagnosticCoverage::None
                && (self.console.errors != 0 || self.console.warnings != 0))
            || (self.network.coverage == BrowserDiagnosticCoverage::None
                && (self.network.requests != 0 || self.network.failed_requests != 0))
            || !valid_sha256(&self.console.transcript_sha256)
            || !valid_sha256(&self.network.transcript_sha256)
        {
            return Err(BrowserEvidenceError::Summary);
        }
        Ok(())
    }

    /// Canonical digest bound into review and delivery previews. Struct field
    /// order is stable and all unordered caller input is rejected or retained
    /// in explicit order, so equal evidence produces an equal digest.
    pub fn digest(&self) -> Result<String, BrowserEvidenceError> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(super::sha256)
            .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))
    }
}

/// Publish one immutable blob beneath Agentum's data directory. Every path
/// component is checked without following links; an existing digest is reused
/// only when its bytes match exactly.
pub fn persist_blob(
    bytes: &[u8],
    media_type: &str,
) -> Result<StoredEvidenceBlob, BrowserEvidenceError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_CAPTURE_BYTES || media_type.is_empty() {
        return Err(BrowserEvidenceError::Capture);
    }
    let sha256 = super::sha256(bytes);
    let relative = format!("evidence/blobs/sha256/{}/{sha256}", &sha256[..2]);
    let root = agentum_store::paths::sdd_evidence_dir()
        .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
    let blob_root = root.join("blobs").join("sha256").join(&sha256[..2]);
    create_owned_directory_chain(&root, &blob_root)?;
    let path = blob_root.join(&sha256);
    match super::artifacts::read_bytes(&path) {
        Ok((stored, stored_hash)) => {
            if stored_hash != sha256 || stored != bytes {
                return Err(BrowserEvidenceError::CaptureMismatch);
            }
        }
        Err(super::artifacts::ArtifactError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            super::artifacts::atomic_write(&path, bytes, Some(super::artifacts::MISSING_HASH))
                .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
        }
        Err(error) => return Err(BrowserEvidenceError::Encoding(error.to_string())),
    }
    Ok(StoredEvidenceBlob {
        sha256,
        byte_length: bytes.len() as i64,
        media_type: media_type.to_owned(),
        storage_relative_path: relative,
    })
}

pub fn read_blob(
    relative_path: &str,
    expected_hash: &str,
) -> Result<Vec<u8>, BrowserEvidenceError> {
    if !valid_sha256(expected_hash)
        || relative_path
            != format!(
                "evidence/blobs/sha256/{}/{expected_hash}",
                &expected_hash[..2]
            )
    {
        return Err(BrowserEvidenceError::Capture);
    }
    let root = agentum_store::paths::data_dir()
        .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
    let path = root.join(relative_path);
    let (bytes, hash) = super::artifacts::read_bytes(&path)
        .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
    if hash != expected_hash || bytes.is_empty() || bytes.len() as u64 > MAX_CAPTURE_BYTES {
        return Err(BrowserEvidenceError::CaptureMismatch);
    }
    Ok(bytes)
}

fn create_owned_directory_chain(root: &Path, target: &Path) -> Result<(), BrowserEvidenceError> {
    let parent = root
        .parent()
        .ok_or_else(|| BrowserEvidenceError::Encoding("evidence root has no parent".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
    let relative = target
        .strip_prefix(parent)
        .map_err(|_| BrowserEvidenceError::Encoding("evidence path escaped data root".into()))?;
    let mut current = PathBuf::from(parent);
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(BrowserEvidenceError::Encoding(
                "unsafe evidence directory component".into(),
            ));
        };
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(BrowserEvidenceError::Encoding(format!(
                    "unsafe evidence directory: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
            }
            Err(error) => return Err(BrowserEvidenceError::Encoding(error.to_string())),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| BrowserEvidenceError::Encoding(error.to_string()))?;
        }
    }
    Ok(())
}

pub fn validate_capture_bytes(
    reference: &BrowserCaptureRef,
    bytes: &[u8],
) -> Result<(), BrowserEvidenceError> {
    validate_capture(reference)?;
    if reference.byte_length != bytes.len() as u64 || reference.sha256 != super::sha256(bytes) {
        return Err(BrowserEvidenceError::CaptureMismatch);
    }
    Ok(())
}

fn validate_target(target: &BrowserTarget) -> Result<(), BrowserEvidenceError> {
    let url =
        reqwest::Url::parse(&target.origin).map_err(|_| BrowserEvidenceError::UnsafeTarget)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || target.origin.trim_end_matches('/') != url.origin().ascii_serialization()
        || target.path.is_empty()
        || target.path.len() > 2048
        || !target.path.starts_with('/')
        || target.path.contains('?')
        || target.path.contains('#')
        || target.path.chars().any(char::is_control)
        || !target.path_redacted
        || !target.query_redacted
    {
        return Err(BrowserEvidenceError::UnsafeTarget);
    }
    Ok(())
}

fn validate_runtime(runtime: &BrowserRuntime) -> Result<(), BrowserEvidenceError> {
    if runtime.name.is_empty()
        || runtime.name.len() > 64
        || !runtime
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || runtime.version.is_empty()
        || runtime.version.len() > 128
        || runtime.version.chars().any(char::is_control)
        || !(1..=16_384).contains(&runtime.viewport_width)
        || !(1..=16_384).contains(&runtime.viewport_height)
        || !(100..=8_000).contains(&runtime.device_scale_milli)
    {
        return Err(BrowserEvidenceError::Runtime);
    }
    Ok(())
}

fn validate_capture(capture: &BrowserCaptureRef) -> Result<(), BrowserEvidenceError> {
    let media_type_valid = match capture.kind {
        BrowserCaptureKind::Screenshot => matches!(
            capture.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        ),
        BrowserCaptureKind::DomSnapshot | BrowserCaptureKind::AccessibilityTree => matches!(
            capture.media_type.as_str(),
            "application/json" | "application/cbor"
        ),
        BrowserCaptureKind::Trace => matches!(
            capture.media_type.as_str(),
            "application/json" | "application/zip" | "application/cbor"
        ),
    };
    if !valid_sha256(&capture.sha256)
        || capture.byte_length == 0
        || capture.byte_length > MAX_CAPTURE_BYTES
        || !media_type_valid
    {
        return Err(BrowserEvidenceError::Capture);
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_assertion_id(value: &str) -> bool {
    let Some(number) = value.strip_prefix("BV-") else {
        return false;
    };
    number.len() == 3 && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_acceptance_criterion(value: &str) -> bool {
    let Some(number) = value.strip_prefix("AC-") else {
        return false;
    };
    !number.is_empty()
        && number.len() <= 16
        && number
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(bytes: &[u8]) -> String {
        super::super::sha256(bytes)
    }

    fn fixture() -> BrowserEvidence {
        let screenshot = b"redacted screenshot fixture";
        BrowserEvidence {
            schema_version: BROWSER_EVIDENCE_SCHEMA_VERSION,
            evidence_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            attempt_id: Uuid::new_v4().to_string(),
            check_id: "browser-login".into(),
            spec_revision: 2,
            captured_at: "2026-07-27T18:30:00Z".into(),
            workspace_fingerprint: hash(b"workspace"),
            target: BrowserTarget {
                origin: "https://example.test".into(),
                path: "/account/session".into(),
                path_redacted: true,
                query_redacted: true,
            },
            browser: BrowserRuntime {
                name: "chromium".into(),
                version: "130.0.1".into(),
                viewport_width: 1440,
                viewport_height: 900,
                device_scale_milli: 1_000,
            },
            captures: vec![BrowserCaptureRef {
                kind: BrowserCaptureKind::Screenshot,
                sha256: hash(screenshot),
                byte_length: screenshot.len() as u64,
                media_type: "image/png".into(),
            }],
            assertions: vec![BrowserAssertion {
                id: "BV-001".into(),
                status: BrowserAssertionStatus::Passed,
                acceptance_criteria: vec!["AC-001".into()],
                evidence_sha256: vec![hash(screenshot)],
            }],
            console: BrowserConsoleSummary {
                coverage: BrowserDiagnosticCoverage::None,
                errors: 0,
                warnings: 0,
                transcript_sha256: hash(b"console redacted"),
            },
            network: BrowserNetworkSummary {
                coverage: BrowserDiagnosticCoverage::MainDocument,
                requests: 12,
                failed_requests: 0,
                transcript_sha256: hash(b"network redacted"),
            },
        }
    }

    #[test]
    fn valid_evidence_is_deterministic_and_capture_is_content_addressed() {
        let evidence = fixture();
        evidence.validate().unwrap();
        assert_eq!(evidence.digest().unwrap(), evidence.digest().unwrap());
        validate_capture_bytes(&evidence.captures[0], b"redacted screenshot fixture").unwrap();
        assert_eq!(
            validate_capture_bytes(&evidence.captures[0], b"tampered"),
            Err(BrowserEvidenceError::CaptureMismatch)
        );
    }

    #[test]
    fn raw_query_credentials_and_unredacted_paths_are_rejected() {
        for origin in [
            "https://user:secret@example.test",
            "https://example.test/?token=secret",
            "file:///tmp/page.html",
        ] {
            let mut evidence = fixture();
            evidence.target.origin = origin.into();
            assert_eq!(evidence.validate(), Err(BrowserEvidenceError::UnsafeTarget));
        }
        let mut evidence = fixture();
        evidence.target.path = "/account?token=secret".into();
        assert_eq!(evidence.validate(), Err(BrowserEvidenceError::UnsafeTarget));
        evidence.target.path = "/account".into();
        evidence.target.path_redacted = false;
        assert_eq!(evidence.validate(), Err(BrowserEvidenceError::UnsafeTarget));
    }

    #[test]
    fn assertions_must_reference_known_captures_and_acceptance_criteria() {
        let mut evidence = fixture();
        evidence.assertions[0].evidence_sha256 = vec![hash(b"unknown")];
        assert_eq!(evidence.validate(), Err(BrowserEvidenceError::Assertion));
        evidence.assertions[0].evidence_sha256 = vec![evidence.captures[0].sha256.clone()];
        evidence.assertions[0].acceptance_criteria = vec!["RQ-001".into()];
        assert_eq!(evidence.validate(), Err(BrowserEvidenceError::Assertion));
    }

    #[test]
    fn serde_rejects_raw_console_fields_and_unknown_contract_extensions() {
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["console"]["messages"] = serde_json::json!(["secret"]);
        assert!(serde_json::from_value::<BrowserEvidence>(value).is_err());
    }
}
