use serde::{Deserialize, Serialize};

/// Metadata only: capturing a destination does not authorize reading its text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    pub token: Option<String>,
    pub writable: Option<bool>,
    pub secure: Option<bool>,
}

impl Destination {
    fn has_identity(&self) -> bool {
        self.token.as_ref().is_some_and(|token| !token.trim().is_empty())
    }

    pub fn confirmed(&self) -> bool {
        self.has_identity() && self.writable == Some(true) && self.secure == Some(false)
    }

    pub fn protected(&self) -> bool {
        self.secure == Some(true) || self.writable == Some(false)
    }

    pub fn unchanged(&self, current: &Self) -> bool {
        self.has_identity()
            && self.token == current.token
            && !self.protected()
            && !current.protected()
            && self.secure == current.secure
            && (!self.confirmed() || current.confirmed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_evidence_is_not_confirmation() {
        assert!(!Destination::default().confirmed());
        assert!(!Destination { token: Some("field".into()), writable: Some(true), secure: None }.confirmed());
        for token in ["", " \t"] {
            let invalid = Destination { token: Some(token.into()), writable: Some(true), secure: Some(false) };
            assert!(!invalid.confirmed());
            assert!(!invalid.unchanged(&invalid));
        }
    }

    #[test]
    fn control_changes_and_security_changes_are_not_safe() {
        let original = Destination { token: Some("a".into()), writable: Some(true), secure: Some(false) };
        assert!(original.unchanged(&original));
        assert!(!original.unchanged(&Destination { token: Some("b".into()), ..original.clone() }));
        let secure = Destination { secure: Some(true), ..original.clone() };
        assert!(secure.protected());
        assert!(!original.unchanged(&secure));
        assert!(Destination { writable: Some(false), ..Default::default() }.protected());
    }
}
