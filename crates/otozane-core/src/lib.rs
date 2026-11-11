//! Platform-independent audio engine for Otozane.
//!
//! The engine has two halves: [`engine::Engine`] runs on the real-time audio
//! thread and never allocates, locks, or performs I/O; [`engine::Controller`]
//! lives on a normal thread, builds effect instances, and hands them over
//! through lock-free queues.

pub mod config;
pub mod dsp;
pub mod effects;
pub mod engine;
pub mod param;
pub mod protocol;

/// Internal processing sample rate. RNNoise requires exactly 48 kHz.
pub const SAMPLE_RATE: u32 = 48_000;
