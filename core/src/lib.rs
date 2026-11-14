//! Core library for `omk` (oh my knowledge).
//!
//! Provides the pure domain entities, storage abstraction traits (`DocumentStore`, `IndexStore`),
//! and CQRS pipelines (`UpsertKnowledgeUseCase`, `SearchKnowledgeUseCase`, `ReadKnowledgeUseCase`).

pub mod domain;
pub mod index;
pub mod search;
pub mod vault;
