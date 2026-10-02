#![forbid(unsafe_code)]

pub mod application;
pub mod bootstrap;
pub mod config;
pub mod domain;

/// Interface adapters are added with their implementation phases.
pub mod interface;

/// Infrastructure adapters are added with their implementation phases.
pub mod infrastructure;
