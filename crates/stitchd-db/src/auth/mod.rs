//! Auth database repositories.
//!
//! Provides [`RefreshTokenRepository`] trait and [`PgRefreshTokenRepository`]
//! implementation backed by PostgreSQL.

pub mod refresh_tokens;

pub use refresh_tokens::{PgRefreshTokenRepository, RefreshTokenRepository};
