//! Shared offset-pagination types for REST list endpoints.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Deserialize a u32 from either a JSON integer or a URL query string value.
/// serde_urlencoded (used by axum's Query extractor) passes all values as strings,
/// which causes `u32::deserialize` to fail when the field is inside a `#[serde(flatten)]`
/// struct. This visitor accepts both representations.
fn de_u32_from_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = u32;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a u32 or string containing a u32")
        }
        fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<u32, E> {
            Ok(v)
        }
        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<u32, E> {
            u32::try_from(v).map_err(|_| E::custom("u32 overflow"))
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<u32, E> {
            v.parse::<u32>().map_err(E::custom)
        }
    }
    d.deserialize_any(Visitor)
}

const DEFAULT_PAGE: u32 = 1;
const DEFAULT_PER_PAGE: u32 = 50;
const MAX_PER_PAGE: u32 = 200;

/// Query parameters for paginated list endpoints.
///
/// Extracted from `?page=N&per_page=N`. Absent params default to
/// `page=1` and `per_page=50`; `per_page` is capped at 200.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PaginationParams {
    #[serde(default, deserialize_with = "de_u32_from_str")]
    pub page: u32,
    #[serde(default, deserialize_with = "de_u32_from_str")]
    pub per_page: u32,
}

impl PaginationParams {
    /// Return the effective page number (minimum 1).
    pub fn effective_page(&self) -> u32 {
        if self.page == 0 {
            DEFAULT_PAGE
        } else {
            self.page
        }
    }

    /// Return the effective per_page value, capped at 200.
    pub fn effective_per_page(&self) -> u32 {
        match self.per_page {
            0 => DEFAULT_PER_PAGE,
            n => n.min(MAX_PER_PAGE),
        }
    }

    /// Compute the SQL OFFSET for the effective page and per_page.
    pub fn offset(&self) -> u64 {
        let page = self.effective_page() as u64;
        let per_page = self.effective_per_page() as u64;
        (page - 1) * per_page
    }

    /// Return the SQL LIMIT (= effective_per_page).
    pub fn limit(&self) -> u64 {
        self.effective_per_page() as u64
    }
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            page: DEFAULT_PAGE,
            per_page: DEFAULT_PER_PAGE,
        }
    }
}

/// Generic paginated response wrapper.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    /// The items on the current page.
    pub items: Vec<T>,
    /// Total number of items across all pages.
    pub total: u64,
    /// Current page number (1-based).
    pub page: u32,
    /// Items per page (effective value after cap).
    pub per_page: u32,
}

impl<T: Serialize> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, total: u64, params: &PaginationParams) -> Self {
        Self {
            items,
            total,
            page: params.effective_page(),
            per_page: params.effective_per_page(),
        }
    }
}

// ===========================================================================
// Cursor (keyset) pagination — platform_hardening_20260608 Phase 4
// ===========================================================================
//
// The guidelines-mandated contract that supersedes the page/per_page envelope
// above (see conductor/tech-stack.md). Request: `?cursor=<opaque>&limit=N`.
// Response: `{ items, next_cursor }`. The opaque token is base64url(JSON(keyset
// position)) — the last returned row's stable sort key(s). Keyset pagination is
// O(1) per page and stable under concurrent inserts, at the cost of no random
// page-number access.

use base64::Engine as _;

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

/// Error decoding an opaque cursor token (malformed base64 / JSON).
#[derive(Debug, thiserror::Error)]
pub enum CursorError {
    /// The token was not valid base64url.
    #[error("invalid cursor encoding")]
    Decode,
    /// The decoded bytes were not the expected keyset JSON shape.
    #[error("invalid cursor payload")]
    Payload,
}

/// Encode a keyset position into an opaque cursor token: `base64url(JSON(key))`.
/// Clients treat the result as opaque.
#[must_use]
pub fn encode_cursor<K: Serialize>(key: &K) -> String {
    let json = serde_json::to_vec(key).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

/// Decode an opaque cursor token back into its keyset position.
///
/// # Errors
/// [`CursorError::Decode`] if the token is not valid base64url;
/// [`CursorError::Payload`] if the bytes are not the expected keyset JSON.
pub fn decode_cursor<K: serde::de::DeserializeOwned>(token: &str) -> Result<K, CursorError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| CursorError::Decode)?;
    serde_json::from_slice(&bytes).map_err(|_| CursorError::Payload)
}

/// Query parameters for cursor-paginated list endpoints (`?cursor=&limit=`).
///
/// Absent `cursor` → first page; absent `limit` → 50, capped at 200.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct CursorParams {
    /// Opaque keyset cursor from a previous response's `next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size (capped at 200).
    #[serde(default, deserialize_with = "de_opt_u32_from_str")]
    pub limit: Option<u32>,
}

/// Like [`de_u32_from_str`] but for the optional `limit` field.
fn de_opt_u32_from_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(Some(de_u32_from_str(d)?))
}

impl CursorParams {
    /// Effective page size (default 50, capped at 200).
    #[must_use]
    pub fn effective_limit(&self) -> u32 {
        match self.limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(n) => n.min(MAX_LIMIT),
        }
    }

    /// Decode the opaque cursor into the repo's keyset position type, if present.
    ///
    /// # Errors
    /// Propagates [`CursorError`] when a present cursor is malformed.
    pub fn decode<K: serde::de::DeserializeOwned>(&self) -> Result<Option<K>, CursorError> {
        match &self.cursor {
            None => Ok(None),
            Some(tok) => decode_cursor(tok).map(Some),
        }
    }
}

/// Generic cursor-paginated response: `{ items, next_cursor }`.
#[derive(Debug, Serialize)]
pub struct CursorPage<T: Serialize> {
    /// Items on this page (at most `limit`).
    pub items: Vec<T>,
    /// Opaque cursor for the next page, or `null` on the last page.
    pub next_cursor: Option<String>,
}

impl<T: Serialize> CursorPage<T> {
    /// Build a page from rows fetched with the **fetch-`limit+1`** idiom.
    ///
    /// Callers query `effective_limit() + 1` rows: if the extra row is present
    /// there is a next page, so the surplus row is dropped and the new last
    /// row's keyset key is encoded into `next_cursor`. `key_of` extracts the
    /// keyset position from an item.
    pub fn from_overfetch<K: Serialize>(
        mut rows: Vec<T>,
        limit: usize,
        key_of: impl Fn(&T) -> K,
    ) -> Self {
        let has_more = rows.len() > limit;
        if has_more {
            rows.truncate(limit);
        }
        let next_cursor = if has_more {
            rows.last().map(|last| encode_cursor(&key_of(last)))
        } else {
            None
        };
        Self {
            items: rows,
            next_cursor,
        }
    }

    /// Build a page from a **keyset backend** that already returned the opaque
    /// next-cursor token (empty string ⇒ last page). The gateway forwards the
    /// token untouched — the owning service's repo owns its format.
    #[must_use]
    pub fn from_token(items: Vec<T>, next_cursor: String) -> Self {
        Self {
            items,
            next_cursor: (!next_cursor.is_empty()).then_some(next_cursor),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn params(page: u32, per_page: u32) -> PaginationParams {
        PaginationParams { page, per_page }
    }

    // ── Cursor pagination ───────────────────────────────────────────────────

    #[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
    struct Key {
        created_at: i64,
        id: u32,
    }

    #[test]
    fn cursor_roundtrips_through_opaque_token() {
        let k = Key {
            created_at: 1_700_000_000,
            id: 42,
        };
        let tok = encode_cursor(&k);
        // Opaque: not the raw JSON.
        assert!(!tok.contains("created_at"));
        let back: Key = decode_cursor(&tok).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn decode_rejects_garbage_token() {
        assert!(matches!(
            decode_cursor::<Key>("!!!not-base64!!!"),
            Err(CursorError::Decode)
        ));
        // Valid base64 but wrong JSON shape → Payload error.
        let bad = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"\"a string\"");
        assert!(matches!(
            decode_cursor::<Key>(&bad),
            Err(CursorError::Payload)
        ));
    }

    #[test]
    fn cursor_limit_defaults_and_caps() {
        let none = CursorParams {
            cursor: None,
            limit: None,
        };
        assert_eq!(none.effective_limit(), 50);
        assert_eq!(
            CursorParams {
                cursor: None,
                limit: Some(0)
            }
            .effective_limit(),
            50
        );
        assert_eq!(
            CursorParams {
                cursor: None,
                limit: Some(500)
            }
            .effective_limit(),
            200
        );
        assert_eq!(
            CursorParams {
                cursor: None,
                limit: Some(75)
            }
            .effective_limit(),
            75
        );
    }

    #[test]
    fn cursor_params_decode_present_and_absent() {
        let k = Key {
            created_at: 1,
            id: 2,
        };
        let with = CursorParams {
            cursor: Some(encode_cursor(&k)),
            limit: None,
        };
        assert_eq!(with.decode::<Key>().unwrap(), Some(k));
        let without = CursorParams {
            cursor: None,
            limit: None,
        };
        assert_eq!(without.decode::<Key>().unwrap(), None);
    }

    #[test]
    fn page_from_overfetch_sets_next_cursor_when_more() {
        // limit=2, fetched 3 (overfetch) → next_cursor points at item #2.
        let rows = vec![
            Key {
                created_at: 1,
                id: 1,
            },
            Key {
                created_at: 2,
                id: 2,
            },
            Key {
                created_at: 3,
                id: 3,
            },
        ];
        let page = CursorPage::from_overfetch(rows, 2, |k| k.clone());
        assert_eq!(page.items.len(), 2, "surplus row dropped");
        let next = page.next_cursor.expect("has a next page");
        let key: Key = decode_cursor(&next).unwrap();
        assert_eq!(key.id, 2, "cursor is the last returned row");
    }

    #[test]
    fn page_from_overfetch_no_next_on_last_page() {
        let rows = vec![Key {
            created_at: 1,
            id: 1,
        }];
        let page = CursorPage::from_overfetch(rows, 2, |k| k.clone());
        assert_eq!(page.items.len(), 1);
        assert!(
            page.next_cursor.is_none(),
            "fewer than limit+1 rows ⇒ last page"
        );
    }

    #[test]
    fn defaults_applied_when_both_zero() {
        let p = params(0, 0);
        assert_eq!(p.effective_page(), 1);
        assert_eq!(p.effective_per_page(), 50);
    }

    #[test]
    fn page_zero_normalised_to_one() {
        let p = params(0, 20);
        assert_eq!(p.effective_page(), 1);
    }

    #[test]
    fn per_page_capped_at_200() {
        let p = params(1, 500);
        assert_eq!(p.effective_per_page(), 200);
    }

    #[test]
    fn per_page_exactly_200_is_allowed() {
        let p = params(1, 200);
        assert_eq!(p.effective_per_page(), 200);
    }

    #[test]
    fn per_page_201_is_capped() {
        let p = params(1, 201);
        assert_eq!(p.effective_per_page(), 200);
    }

    #[test]
    fn offset_page_1() {
        let p = params(1, 10);
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn offset_page_2() {
        let p = params(2, 10);
        assert_eq!(p.offset(), 10);
    }

    #[test]
    fn offset_page_3() {
        let p = params(3, 25);
        assert_eq!(p.offset(), 50);
    }

    #[test]
    fn limit_returns_effective_per_page() {
        let p = params(1, 75);
        assert_eq!(p.limit(), 75);
    }

    #[test]
    fn paginated_response_sets_correct_fields() {
        let p = params(2, 10);
        let resp: PaginatedResponse<u32> = PaginatedResponse::new(vec![1, 2, 3], 30, &p);
        assert_eq!(resp.page, 2);
        assert_eq!(resp.per_page, 10);
        assert_eq!(resp.total, 30);
        assert_eq!(resp.items.len(), 3);
    }

    #[test]
    fn paginated_response_uses_effective_page_and_per_page() {
        // page=0 → effective 1; per_page=0 → effective 50
        let p = params(0, 0);
        let resp: PaginatedResponse<u32> = PaginatedResponse::new(vec![], 0, &p);
        assert_eq!(resp.page, 1);
        assert_eq!(resp.per_page, 50);
    }
}
