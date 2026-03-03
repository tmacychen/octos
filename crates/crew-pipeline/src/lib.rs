//! DOT-based pipeline orchestration engine for crew-rs.
//!
//! Parse DOT graphs with typed attributes, walk the graph with async handlers,
//! and execute multi-step agent workflows with per-node model selection.

pub mod condition;
pub mod discovery;
pub mod executor;
pub mod graph;
pub mod handler;
pub mod parser;
pub mod tool;
pub mod validate;

pub use executor::{PipelineExecutor, PipelineResult, PipelineStatusBridge};
pub use graph::{
    HandlerKind, NodeOutcome, OutcomeStatus, PipelineEdge, PipelineGraph, PipelineNode,
};
pub use handler::{
    CodergenHandler, GateHandler, Handler, HandlerRegistry, NoopHandler, ShellHandler,
};
pub use parser::parse_dot;
pub use tool::RunPipelineTool;
pub use validate::{LintDiagnostic, Severity, validate};
