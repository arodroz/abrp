//! Routing Engine: contraction-hierarchy road-graph queries.
//!
//! `reference` is a plain Dijkstra over the base graph, kept as test ground
//! truth. `Router` is the production CH query kernel over an open `Rpack`.

pub mod reference;
mod router;

pub use router::{Route, Router};
