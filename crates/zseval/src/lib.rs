//! zseval — eval harness for zerostack agents.
//!
//! Modules mirror the pipeline: scenario (what to test) -> backend (drive the
//! real agent) -> transcript (normalize the evidence) -> asserts (deterministic
//! floor) -> judge (subjective layer) -> verdict (three-value grading) ->
//! compare (regression gate).

pub mod asserts;
pub mod backend;
pub mod compare;
pub mod domains;
pub mod judge;
pub mod matrix;
pub mod prompts;
pub mod runner;
pub mod scenario;
pub mod seed;
pub mod target;
pub mod transcript;
pub mod util;
pub mod verdict;
