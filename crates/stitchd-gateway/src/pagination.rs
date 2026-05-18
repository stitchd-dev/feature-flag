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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn params(page: u32, per_page: u32) -> PaginationParams {
        PaginationParams { page, per_page }
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
