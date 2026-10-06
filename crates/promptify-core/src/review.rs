use std::time::Instant;

use crate::pipeline::{BackendError, CancelToken};

pub struct ReviewRequest<'a> {
    pub transcript: &'a str,
    pub draft: &'a str,
    pub deadline: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Skipped,
    Approved,
    Revise { issues: Vec<String> },
    Uncertain { detail: String },
}

pub trait PromptReviewer: Send + Sync {
    fn enabled(&self) -> bool {
        true
    }

    fn review(&self, request: &ReviewRequest<'_>, cancel: &CancelToken) -> Result<ReviewDecision, BackendError>;
}
