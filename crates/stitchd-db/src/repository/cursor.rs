//! Opaque keyset-cursor tokens for list pagination (feature-flag-cj5).
//!
//! Every top-level list query orders by `created_at` with `id` as the unique
//! tiebreaker, so the keyset position is `(created_at, id)`. Repos page with
//!
//! ```sql
//! WHERE ($cursor IS NULL OR (created_at, id) > ($cursor_created_at, $cursor_id))
//! ORDER BY created_at, id
//! LIMIT $n + 1
//! ```
//!
//! and fetch `limit + 1` rows: the surplus row means there is a next page, so it
//! is dropped and the new last row's `(created_at, id)` is encoded into the next
//! cursor. This is O(1) per page (no deep-`OFFSET` scan) and stable under
//! concurrent inserts.
//!
//! The token is `base64url(JSON({created_at, id}))` — clients (and the gateway,
//! which only forwards it) treat it as opaque. The format is owned here, next to
//! the SQL that produces and consumes it.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A keyset position: the `(created_at, id)` of a row in a `created_at, id`
/// ordered list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysetCursor {
    /// `created_at` of the boundary row.
    pub created_at: DateTime<Utc>,
    /// `id` of the boundary row (the unique tiebreaker).
    pub id: Uuid,
}

/// Error decoding an opaque keyset-cursor token.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CursorError {
    /// The token was not valid base64url.
    #[error("invalid cursor encoding")]
    Decode,
    /// The decoded bytes were not the expected `{created_at, id}` JSON.
    #[error("invalid cursor payload")]
    Payload,
}

impl KeysetCursor {
    /// Encode this position into an opaque `base64url(JSON)` token.
    #[must_use]
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    /// Decode an opaque token back into a keyset position.
    ///
    /// # Errors
    /// [`CursorError::Decode`] for non-base64url input; [`CursorError::Payload`]
    /// when the bytes are not the expected `{created_at, id}` JSON.
    pub fn decode(token: &str) -> Result<Self, CursorError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| CursorError::Decode)?;
        serde_json::from_slice(&bytes).map_err(|_| CursorError::Payload)
    }

    /// Decode an optional token: an empty / absent token is the first page
    /// (`Ok(None)`); a present token is decoded.
    ///
    /// # Errors
    /// Propagates [`CursorError`] when a present token is malformed.
    pub fn decode_opt(token: Option<&str>) -> Result<Option<Self>, CursorError> {
        match token {
            None | Some("") => Ok(None),
            Some(t) => Self::decode(t).map(Some),
        }
    }
}

/// Clamp a requested page size into `[1, max]`, defaulting `0` to `default`.
#[must_use]
pub fn effective_limit(requested: u32, default: u32, max: u32) -> u32 {
    match requested {
        0 => default,
        n => n.min(max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur(secs: i64, id: u128) -> KeysetCursor {
        KeysetCursor {
            created_at: DateTime::from_timestamp(secs, 0).unwrap(),
            id: Uuid::from_u128(id),
        }
    }

    #[test]
    fn roundtrips_through_opaque_token() {
        let c = cur(1_700_000_000, 42);
        let tok = c.encode();
        assert!(!tok.contains("created_at"), "token is opaque");
        assert_eq!(KeysetCursor::decode(&tok).unwrap(), c);
    }

    #[test]
    fn decode_opt_handles_absent_empty_and_present() {
        assert_eq!(KeysetCursor::decode_opt(None).unwrap(), None);
        assert_eq!(KeysetCursor::decode_opt(Some("")).unwrap(), None);
        let c = cur(1, 2);
        assert_eq!(
            KeysetCursor::decode_opt(Some(&c.encode())).unwrap(),
            Some(c)
        );
    }

    #[test]
    fn decode_rejects_garbage() {
        assert_eq!(KeysetCursor::decode("!!!"), Err(CursorError::Decode));
        let bad = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"\"str\"");
        assert_eq!(KeysetCursor::decode(&bad), Err(CursorError::Payload));
    }

    #[test]
    fn effective_limit_defaults_and_caps() {
        assert_eq!(effective_limit(0, 50, 200), 50);
        assert_eq!(effective_limit(500, 50, 200), 200);
        assert_eq!(effective_limit(75, 50, 200), 75);
    }
}
