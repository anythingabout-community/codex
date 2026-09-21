use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

/// One bounded part of the stable Supervisor or Implementer role policy.
pub struct SupervisorRoleFragment(String);

impl SupervisorRoleFragment {
    pub fn new(text: &str) -> Self {
        Self(text[..text.floor_char_boundary(text.len().min(700))].to_string())
    }
}

impl ContextualUserFragment for SupervisorRoleFragment {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("supervisor.role".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<supervisor_role>", "</supervisor_role>")
    }

    fn body(&self) -> String {
        self.0.clone()
    }
}
