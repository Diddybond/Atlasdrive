//! # Family Archive core
//!
//! Safety-critical service layer for the Family Archive local photo catalogue.
//!
//! Priority order when specifications disagree (see `AGENTS.md`):
//!   1. Safety requirements
//!   2. Definition of done
//!   3. Product specification
//!   4. Architecture decisions
//!   5. Implementation notes
//!
//! Everything the GUI, CLI and verifier need is exposed from here. The module
//! layout mirrors the architecture layers in `docs/03_ARCHITECTURE.md`.

pub mod ai;
pub mod config;
pub mod crypto;
pub mod dates;
pub mod db;
pub mod drive;
pub mod error;
pub mod faces;
pub mod integrity;
pub mod logging;
pub mod net;
pub mod pipeline;
pub mod progress;
pub mod queue;
pub mod scan;
pub mod search;
pub mod util;
pub mod verifier;

pub use config::{AppPaths, Config};
pub use error::{Error, Result};
