use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

/// Bounded runtime evidence or steering for Continuous Planning.
/// Each fragment is capped in bytes, including for byte-level tokenization.
pub struct ContinuousPlanningFragment(String);

impl ContinuousPlanningFragment {
    pub fn new(text: &str) -> Self {
        let mut end = text.len().min(700);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self(text[..end].to_string())
    }
}

impl ContextualUserFragment for ContinuousPlanningFragment {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("continuous_planning.context".to_string())
    }
    fn role(&self) -> &'static str {
        "user"
    }
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }
    fn type_markers() -> (&'static str, &'static str) {
        (
            "<continuous_planning_context>",
            "</continuous_planning_context>",
        )
    }
    fn body(&self) -> String {
        self.0.clone()
    }
}
