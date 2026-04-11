//! Database access layer: sqlx queries, migrations, and repository implementations.
//!
//! All SQL queries use compile-time checked `sqlx::query!` / `sqlx::query_as!` macros.
//! Schema changes are managed via migrations in `migrations/`.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

// TODO: repository traits and implementations
// TODO: connection pool setup

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
