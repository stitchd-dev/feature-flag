//! Email delivery service with SMTP + offline-link fallback.
//!
//! Configuration is driven by environment variables:
//! - `SMTP_HOST` — SMTP server hostname
//! - `SMTP_PORT` — SMTP port (default 587)
//! - `SMTP_USER` — SMTP username
//! - `SMTP_PASSWORD` — SMTP password
//! - `SMTP_FROM` — From address
//! - `EMAIL_REQUIRED` — if `"true"`, return an error when SMTP is not configured

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use thiserror::Error;

/// A link that can be shared out-of-band when email is unavailable.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OfflineLink {
    /// The URL the user should visit.
    pub url: String,
    /// Human-readable description of the link.
    pub description: String,
}

/// SMTP connection parameters.
struct SmtpConfig {
    host: String,
    port: u16,
    user: String,
    password: String,
    from: String,
}

/// Email delivery service.
pub struct EmailService {
    smtp_config: Option<SmtpConfig>,
    email_required: bool,
}

impl EmailService {
    /// Build from environment variables.
    ///
    /// SMTP is considered configured only when **all** of `SMTP_HOST`, `SMTP_USER`,
    /// `SMTP_PASSWORD`, and `SMTP_FROM` are present.  `SMTP_PORT` defaults to 587.
    /// `EMAIL_REQUIRED` defaults to `false`.
    #[must_use]
    pub fn from_env() -> Self {
        let host = std::env::var("SMTP_HOST").ok();
        let user = std::env::var("SMTP_USER").ok();
        let password = std::env::var("SMTP_PASSWORD").ok();
        let from = std::env::var("SMTP_FROM").ok();
        let port: u16 = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(587);
        let email_required = std::env::var("EMAIL_REQUIRED")
            .is_ok_and(|v| v.eq_ignore_ascii_case("true"));

        let smtp_config = match (host, user, password, from) {
            (Some(host), Some(user), Some(password), Some(from)) => Some(SmtpConfig {
                host,
                port,
                user,
                password,
                from,
            }),
            _ => None,
        };

        Self {
            smtp_config,
            email_required,
        }
    }

    /// Send an email.
    ///
    /// * If SMTP is configured, sends the email and returns `Ok(None)`.
    /// * If SMTP is **not** configured and `EMAIL_REQUIRED=false`, returns `Ok(Some(OfflineLink))`
    ///   containing the `offline_url` so the caller can surface it another way.
    /// * If SMTP is **not** configured and `EMAIL_REQUIRED=true`, returns `Err(EmailError::NotConfigured)`.
    ///
    /// # Errors
    /// Returns [`EmailError::NotConfigured`] or [`EmailError::Smtp`] on failure.
    pub async fn send(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        offline_url: &str,
    ) -> Result<Option<OfflineLink>, EmailError> {
        let Some(cfg) = &self.smtp_config else {
            if self.email_required {
                return Err(EmailError::NotConfigured);
            }
            return Ok(Some(OfflineLink {
                url: offline_url.to_string(),
                description: format!("Email delivery unavailable. Use this link: {offline_url}"),
            }));
        };

        let email = Message::builder()
            .from(
                cfg.from
                    .parse()
                    .map_err(|e: lettre::address::AddressError| EmailError::Smtp(e.to_string()))?,
            )
            .to(to
                .parse()
                .map_err(|e: lettre::address::AddressError| EmailError::Smtp(e.to_string()))?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| EmailError::Smtp(e.to_string()))?;

        let creds = Credentials::new(cfg.user.clone(), cfg.password.clone());
        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
                .map_err(|e| EmailError::Smtp(e.to_string()))?
                .port(cfg.port)
                .credentials(creds)
                .build();

        mailer
            .send(email)
            .await
            .map_err(|e| EmailError::Smtp(e.to_string()))?;

        Ok(None)
    }
}

/// Errors from the email delivery service.
#[derive(Debug, Error)]
pub enum EmailError {
    /// SMTP is not configured but `EMAIL_REQUIRED=true`.
    #[error("Email service required but not configured")]
    NotConfigured,
    /// An SMTP protocol or delivery error.
    #[error("SMTP error: {0}")]
    Smtp(String),
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_smtp_env() {
        for key in &[
            "SMTP_HOST",
            "SMTP_PORT",
            "SMTP_USER",
            "SMTP_PASSWORD",
            "SMTP_FROM",
            "EMAIL_REQUIRED",
        ] {
            // SAFETY: tests run single-threaded within this module
            unsafe { std::env::remove_var(key) };
        }
    }

    #[test]
    fn from_env_with_no_vars_creates_unconfigured_service() {
        clear_smtp_env();
        let svc = EmailService::from_env();
        assert!(svc.smtp_config.is_none());
        assert!(!svc.email_required);
    }

    #[tokio::test]
    async fn send_without_smtp_and_not_required_returns_offline_link() {
        clear_smtp_env();
        let svc = EmailService::from_env();
        let result = svc
            .send(
                "user@example.com",
                "Test",
                "body",
                "https://example.com/accept",
            )
            .await
            .expect("should not error");
        let link = result.expect("should return OfflineLink");
        assert_eq!(link.url, "https://example.com/accept");
    }

    #[tokio::test]
    async fn send_without_smtp_and_required_returns_err() {
        clear_smtp_env();
        // SAFETY: tests run single-threaded within this module
        unsafe { std::env::set_var("EMAIL_REQUIRED", "true") };
        let svc = EmailService::from_env();
        let result = svc
            .send(
                "user@example.com",
                "Test",
                "body",
                "https://example.com/accept",
            )
            .await;
        assert!(matches!(result, Err(EmailError::NotConfigured)));
        // SAFETY: tests run single-threaded within this module
        unsafe { std::env::remove_var("EMAIL_REQUIRED") };
    }
}
