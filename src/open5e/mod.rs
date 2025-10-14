//! Open5e v1 upstream: HTTP client and models.

pub mod client;
pub mod models;

pub use client::{Client, Fetched, UpstreamError};
pub use models::{Action, Monster, SearchHit, SearchResults, Spell};
