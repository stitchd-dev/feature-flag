//! SAML 2.0 Service Provider (SP) engine.
//!
//! Implements SP-initiated SSO, ACS response validation, SP metadata generation,
//! and IdP-initiated Single Logout (SLO) using XML string building and `quick-xml`
//! parsing.
//!
//! # Signature validation note
//! Full XML digital signature verification (xmldsig) requires native library
//! support (`libxmlsec1`). This implementation parses and validates the
//! structure of SAML messages but **does not cryptographically verify the IdP
//! signature**. A TODO is placed where signature verification should occur.
//! For production use, integrate a crate such as `samael` (with `xmlsec` feature)
//! or `xmlsec` directly.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use flate2::Compression;
use flate2::write::DeflateEncoder;
use std::io::Write;
use thiserror::Error;
use uuid::Uuid;

/// Errors produced by the SAML engine.
#[derive(Debug, Error)]
pub enum SamlError {
    /// A required XML element or attribute was missing.
    #[error("missing required XML element or attribute: {0}")]
    MissingElement(String),

    /// The SAML response failed structural validation.
    #[error("invalid SAML response: {0}")]
    InvalidResponse(String),

    /// Base64 decoding failed.
    #[error("base64 decode error: {0}")]
    Base64(String),

    /// XML parsing failed.
    #[error("XML parse error: {0}")]
    Xml(String),

    /// Deflate compression failed.
    #[error("deflate error: {0}")]
    Deflate(String),
}

/// Configuration for a SAML 2.0 Service Provider.
pub struct SamlConfig {
    /// IdP entity ID (Issuer) from IdP metadata.
    pub entity_id: String,
    /// IdP Single Sign-On URL (redirect binding endpoint).
    pub sso_url: String,
    /// IdP signing certificate in PEM format (used for signature validation).
    pub idp_cert_pem: String,
    /// Our SP entity ID (must match what's configured in the IdP).
    pub sp_entity_id: String,
}

/// SAML 2.0 Service Provider.
pub struct SamlProvider {
    config: SamlConfig,
}

/// Context extracted from a SAML IdP-initiated LogoutRequest.
#[derive(Debug, Clone)]
pub struct SloContext {
    /// NameID of the user being logged out (usually their email).
    pub name_id: String,
    /// SAML session index, if provided.
    pub session_index: Option<String>,
}

impl SamlProvider {
    /// Create a new `SamlProvider` from the given configuration.
    #[must_use]
    pub fn new(config: SamlConfig) -> Self {
        Self { config }
    }

    /// Build an AuthnRequest XML and the redirect URL with `SAMLRequest` param.
    ///
    /// # Returns
    /// A tuple of `(authn_request_xml, redirect_url)` where `redirect_url` is
    /// the IdP SSO URL with a deflate+base64+URL-encoded `SAMLRequest` query
    /// parameter and an optional `RelayState` parameter.
    ///
    /// # Errors
    /// Returns [`SamlError::Deflate`] if deflate compression fails.
    pub fn authn_request(
        &self,
        acs_url: &str,
        relay_state: &str,
    ) -> Result<(String, String), SamlError> {
        let id = format!("_{}", Uuid::new_v4().as_simple());
        let issue_instant = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<samlp:AuthnRequest
  xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
  ID="{id}"
  Version="2.0"
  IssueInstant="{issue_instant}"
  Destination="{sso_url}"
  AssertionConsumerServiceURL="{acs_url}"
  ProtocolBinding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST">
  <saml:Issuer>{sp_entity_id}</saml:Issuer>
  <samlp:NameIDPolicy
    Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress"
    AllowCreate="true"/>
</samlp:AuthnRequest>"#,
            sso_url = self.config.sso_url,
            sp_entity_id = self.config.sp_entity_id,
        );

        // Deflate (raw, no zlib header) + base64 + URL-encode per SAML redirect binding spec
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(xml.as_bytes())
            .map_err(|e| SamlError::Deflate(e.to_string()))?;
        let compressed = encoder
            .finish()
            .map_err(|e| SamlError::Deflate(e.to_string()))?;
        let b64 = BASE64.encode(&compressed);
        let encoded = url_encode(&b64);
        let relay_encoded = url_encode(relay_state);

        let redirect_url = format!(
            "{}?SAMLRequest={}&RelayState={}",
            self.config.sso_url, encoded, relay_encoded
        );

        Ok((xml, redirect_url))
    }

    /// Validate a base64-encoded SAML Response (HTTP-POST binding).
    ///
    /// Decodes the base64 payload, parses the XML, and extracts the asserted
    /// email from `NameID` or the `email` attribute.
    ///
    /// # Security note
    /// **XML signature is not cryptographically verified** in this implementation.
    /// See module-level doc for details.
    ///
    /// # Errors
    /// Returns a [`SamlError`] if decoding, parsing, or email extraction fails.
    pub fn validate_response(&self, base64_response: &str) -> Result<String, SamlError> {
        let xml_bytes = BASE64
            .decode(base64_response.trim())
            .map_err(|e| SamlError::Base64(e.to_string()))?;
        let xml = String::from_utf8(xml_bytes)
            .map_err(|e| SamlError::Xml(format!("non-UTF-8 response: {e}")))?;

        // TODO: Verify IdP XML digital signature using libxmlsec1 / samael before
        // trusting the content of the response. Without signature verification this
        // implementation should only be used in trusted network environments or for
        // development/testing.

        extract_email_from_response_xml(&xml)
    }

    /// Generate SP metadata XML for this service provider.
    ///
    /// Returns a standard `EntityDescriptor` XML document suitable for upload to
    /// the IdP or exposure at `/metadata`.
    #[must_use]
    pub fn sp_metadata_xml(&self, acs_url: &str) -> String {
        let valid_until = (Utc::now() + chrono::Duration::days(365))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor
  xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata"
  xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
  entityID="{sp_entity_id}"
  validUntil="{valid_until}">
  <md:SPSSODescriptor
    AuthnRequestsSigned="false"
    WantAssertionsSigned="true"
    protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:AssertionConsumerService
      Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
      Location="{acs_url}"
      index="1"/>
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
  </md:SPSSODescriptor>
</md:EntityDescriptor>"#,
            sp_entity_id = self.config.sp_entity_id,
        )
    }

    /// Parse a SAML LogoutRequest XML and return the `SloContext`.
    ///
    /// Extracts the `NameID` (and optional `SessionIndex`) from an IdP-initiated
    /// SLO request.
    ///
    /// # Errors
    /// Returns a [`SamlError`] if the XML is malformed or missing required fields.
    pub fn validate_logout_request(&self, xml: &str) -> Result<SloContext, SamlError> {
        let name_id = extract_text_between(xml, "<saml:NameID", "</saml:NameID>")
            .or_else(|| extract_text_between(xml, "<NameID", "</NameID>"))
            .ok_or_else(|| SamlError::MissingElement("NameID".to_string()))?;

        let session_index =
            extract_text_between(xml, "<samlp:SessionIndex>", "</samlp:SessionIndex>")
                .or_else(|| extract_text_between(xml, "<SessionIndex>", "</SessionIndex>"));

        Ok(SloContext {
            name_id,
            session_index,
        })
    }

    /// Build a SAML LogoutResponse XML for the given `in_response_to` ID.
    ///
    /// Returns well-formed XML suitable for returning to the IdP in response to
    /// a LogoutRequest.
    #[must_use]
    pub fn logout_response(&self, in_response_to: &str, relay_state: Option<&str>) -> String {
        let id = format!("_{}", Uuid::new_v4().as_simple());
        let issue_instant = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let relay_comment = relay_state
            .map(|rs| format!("<!-- RelayState: {rs} -->"))
            .unwrap_or_default();

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
{relay_comment}
<samlp:LogoutResponse
  xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
  ID="{id}"
  Version="2.0"
  IssueInstant="{issue_instant}"
  InResponseTo="{in_response_to}">
  <saml:Issuer>{sp_entity_id}</saml:Issuer>
  <samlp:Status>
    <samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/>
  </samlp:Status>
</samlp:LogoutResponse>"#,
            sp_entity_id = self.config.sp_entity_id,
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// URL-encode a string (percent-encoding per RFC 3986).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Extract the trimmed text content between two XML markers.
/// Strips element attributes from the opening tag.
fn extract_text_between(xml: &str, open_tag_prefix: &str, close_tag: &str) -> Option<String> {
    let start_pos = xml.find(open_tag_prefix)?;
    // Find the end of the opening tag (could have attributes)
    let tag_end = xml[start_pos..].find('>')?;
    let content_start = start_pos + tag_end + 1;
    let end_pos = xml[content_start..].find(close_tag)?;
    let content = xml[content_start..content_start + end_pos].trim();
    if content.is_empty() {
        None
    } else {
        Some(content.to_string())
    }
}

/// Extract the user email from a SAML Response XML.
///
/// Checks the following locations in order:
/// 1. `<saml:NameID>` or `<NameID>` in the Subject
/// 2. An `Attribute` with `Name="email"` or `Name="emailAddress"`
fn extract_email_from_response_xml(xml: &str) -> Result<String, SamlError> {
    // Try NameID first (most common for email-format NameID)
    if let Some(email) = extract_text_between(xml, "<saml:NameID", "</saml:NameID>")
        .or_else(|| extract_text_between(xml, "<NameID", "</NameID>"))
    {
        if email.contains('@') {
            return Ok(email);
        }
    }

    // Try Attribute values for email
    for attr_name in &[
        "email",
        "emailAddress",
        "mail",
        "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress",
        "urn:oid:1.2.840.113549.1.9.1",
    ] {
        if let Some(email) = find_attribute_value(xml, attr_name) {
            if email.contains('@') {
                return Ok(email);
            }
        }
    }

    Err(SamlError::MissingElement(
        "email (not found in NameID or Attributes)".to_string(),
    ))
}

/// Find the `AttributeValue` for a named SAML `Attribute`.
fn find_attribute_value(xml: &str, attr_name: &str) -> Option<String> {
    // Look for Name="<attr_name>" in the XML
    let search = format!("Name=\"{attr_name}\"");
    let pos = xml.find(&search)?;
    // From that position, find the next AttributeValue
    let rest = &xml[pos..];
    extract_text_between(rest, "<saml:AttributeValue", "</saml:AttributeValue>")
        .or_else(|| extract_text_between(rest, "<AttributeValue", "</AttributeValue>"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_provider() -> SamlProvider {
        SamlProvider::new(SamlConfig {
            entity_id: "https://idp.example.com".to_string(),
            sso_url: "https://idp.example.com/sso".to_string(),
            idp_cert_pem: "CERT_PLACEHOLDER".to_string(),
            sp_entity_id: "https://sp.stitchd.dev".to_string(),
        })
    }

    // -------------------------------------------------------------------------
    // authn_request
    // -------------------------------------------------------------------------

    #[test]
    fn authn_request_returns_xml_with_authn_request_element() {
        let sp = test_provider();
        let (xml, _redirect) = sp
            .authn_request("https://sp.stitchd.dev/acs", "my-relay-state")
            .expect("authn_request should succeed");

        assert!(
            xml.contains("AuthnRequest"),
            "XML should contain AuthnRequest element"
        );
        assert!(
            xml.contains("samlp:AuthnRequest"),
            "XML should use samlp namespace"
        );
        assert!(
            xml.contains("AssertionConsumerServiceURL=\"https://sp.stitchd.dev/acs\""),
            "XML should include ACS URL"
        );
        assert!(
            xml.contains("https://sp.stitchd.dev"),
            "XML should include SP entity ID"
        );
    }

    #[test]
    fn authn_request_redirect_url_contains_saml_request_param() {
        let sp = test_provider();
        let (_xml, redirect) = sp
            .authn_request("https://sp.stitchd.dev/acs", "state123")
            .expect("authn_request should succeed");

        assert!(
            redirect.starts_with("https://idp.example.com/sso"),
            "redirect URL should start with SSO URL"
        );
        assert!(
            redirect.contains("SAMLRequest="),
            "redirect URL should contain SAMLRequest param"
        );
        assert!(
            redirect.contains("RelayState="),
            "redirect URL should contain RelayState param"
        );
    }

    #[test]
    fn authn_request_different_ids_each_call() {
        let sp = test_provider();
        let (xml1, _) = sp
            .authn_request("https://sp.stitchd.dev/acs", "rs")
            .unwrap();
        let (xml2, _) = sp
            .authn_request("https://sp.stitchd.dev/acs", "rs")
            .unwrap();
        // IDs are UUIDs so should differ
        assert_ne!(xml1, xml2, "each AuthnRequest should have a unique ID");
    }

    // -------------------------------------------------------------------------
    // sp_metadata_xml
    // -------------------------------------------------------------------------

    #[test]
    fn sp_metadata_xml_contains_entity_descriptor() {
        let sp = test_provider();
        let metadata = sp.sp_metadata_xml("https://sp.stitchd.dev/acs");

        assert!(
            metadata.contains("EntityDescriptor"),
            "metadata should contain EntityDescriptor"
        );
        assert!(
            metadata.contains("md:EntityDescriptor"),
            "metadata should use md namespace"
        );
        assert!(
            metadata.contains("https://sp.stitchd.dev"),
            "metadata should contain SP entity ID"
        );
        assert!(
            metadata.contains("https://sp.stitchd.dev/acs"),
            "metadata should contain ACS URL"
        );
    }

    #[test]
    fn sp_metadata_xml_contains_sp_sso_descriptor() {
        let sp = test_provider();
        let metadata = sp.sp_metadata_xml("https://sp.stitchd.dev/acs");

        assert!(
            metadata.contains("SPSSODescriptor"),
            "metadata should contain SPSSODescriptor"
        );
        assert!(
            metadata.contains("AssertionConsumerService"),
            "metadata should contain AssertionConsumerService"
        );
    }

    // -------------------------------------------------------------------------
    // logout_response
    // -------------------------------------------------------------------------

    #[test]
    fn logout_response_contains_logout_response_element() {
        let sp = test_provider();
        let xml = sp.logout_response("_req123", Some("relay"));

        assert!(
            xml.contains("LogoutResponse"),
            "should contain LogoutResponse element"
        );
        assert!(
            xml.contains("samlp:LogoutResponse"),
            "should use samlp namespace"
        );
        assert!(
            xml.contains("InResponseTo=\"_req123\""),
            "should contain InResponseTo attribute"
        );
        assert!(
            xml.contains("Success"),
            "should contain Success status code"
        );
    }

    #[test]
    fn logout_response_without_relay_state_is_valid() {
        let sp = test_provider();
        let xml = sp.logout_response("_req456", None);

        assert!(xml.contains("LogoutResponse"));
        assert!(xml.contains("InResponseTo=\"_req456\""));
    }

    // -------------------------------------------------------------------------
    // validate_logout_request
    // -------------------------------------------------------------------------

    const SAMPLE_LOGOUT_REQUEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<samlp:LogoutRequest
  xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
  ID="_logoutreq_001"
  Version="2.0"
  IssueInstant="2024-01-01T00:00:00Z">
  <saml:Issuer>https://idp.example.com</saml:Issuer>
  <saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">user@example.com</saml:NameID>
  <samlp:SessionIndex>session_abc_123</samlp:SessionIndex>
</samlp:LogoutRequest>"#;

    #[test]
    fn validate_logout_request_extracts_name_id() {
        let sp = test_provider();
        let ctx = sp
            .validate_logout_request(SAMPLE_LOGOUT_REQUEST)
            .expect("should parse valid LogoutRequest");

        assert_eq!(ctx.name_id, "user@example.com");
    }

    #[test]
    fn validate_logout_request_extracts_session_index() {
        let sp = test_provider();
        let ctx = sp
            .validate_logout_request(SAMPLE_LOGOUT_REQUEST)
            .expect("should parse valid LogoutRequest");

        assert_eq!(ctx.session_index.as_deref(), Some("session_abc_123"));
    }

    #[test]
    fn validate_logout_request_missing_name_id_returns_error() {
        let sp = test_provider();
        let xml = r#"<samlp:LogoutRequest xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"><saml:Issuer>x</saml:Issuer></samlp:LogoutRequest>"#;
        let result = sp.validate_logout_request(xml);
        assert!(result.is_err(), "should fail without NameID");
    }

    // -------------------------------------------------------------------------
    // validate_response
    // -------------------------------------------------------------------------

    #[test]
    fn validate_response_extracts_name_id_email() {
        let sp = test_provider();
        let xml = r#"<?xml version="1.0"?><samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"><saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"><saml:Subject><saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">test@example.com</saml:NameID></saml:Subject></saml:Assertion></samlp:Response>"#;
        let b64 = BASE64.encode(xml.as_bytes());
        let email = sp
            .validate_response(&b64)
            .expect("should extract email from NameID");
        assert_eq!(email, "test@example.com");
    }

    #[test]
    fn validate_response_no_email_returns_error() {
        let sp = test_provider();
        let xml = r#"<?xml version="1.0"?><samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"><saml:Assertion><saml:Subject><saml:NameID>not-an-email</saml:NameID></saml:Subject></saml:Assertion></samlp:Response>"#;
        let b64 = BASE64.encode(xml.as_bytes());
        let result = sp.validate_response(&b64);
        assert!(result.is_err(), "should fail when no email found");
    }

    #[test]
    fn validate_response_invalid_base64_returns_error() {
        let sp = test_provider();
        let result = sp.validate_response("!!!not-base64!!!");
        assert!(result.is_err());
    }
}
