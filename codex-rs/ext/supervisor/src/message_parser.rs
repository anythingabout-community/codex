//! A bounded, side-effect-free protocol for directed Supervisor messages.
//!
//! The canonical wire form is structural and avoids natural-language `To:` markers:
//!
//! ```text
//! <messages>
//! <user>
//! I am checking the result.
//! </user>
//! <implementer>
//! Verify the selected step.
//! </implementer>
//! </messages>
//! ```
//!
use codex_extension_items::continuous_planning::DirectedMessage;
use codex_extension_items::continuous_planning::MessageBatch;
use codex_extension_items::continuous_planning::MessageRecipient;

pub(crate) const MAX_BATCH_BYTES: usize = 8192;

#[derive(Default)]
pub(crate) struct MessageParser {
    bytes: usize,
    line: String,
    line_number: usize,
    block_number: usize,
    started: bool,
    ended: bool,
    current: Option<MessageRecipient>,
    messages: Vec<DirectedMessage>,
    visible_delta: String,
    visible_markup_candidate: String,
    error: Option<String>,
}

impl codex_extension_api::MessageStreamValidator for MessageParser {
    fn push(&mut self, delta: &str) -> Result<(), String> {
        MessageParser::push(self, delta)
    }

    fn take_visible_delta(&mut self) -> Option<String> {
        (!self.visible_delta.is_empty()).then(|| std::mem::take(&mut self.visible_delta))
    }
}

impl MessageParser {
    pub(crate) fn push(&mut self, delta: &str) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.visible_delta.clear();
        for ch in delta.chars() {
            if ch.len_utf8() > MAX_BATCH_BYTES.saturating_sub(self.bytes) {
                self.line_number += 1;
                return self.fail("batch exceeds 8 KiB");
            }
            self.bytes += ch.len_utf8();
            self.observe_stream_char(ch);
            if ch == '\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.ends_with('\r') {
                    line.pop();
                }
                self.process_line(&line)?;
            } else {
                self.line.push(ch);
            }
        }
        Ok(())
    }

    pub(crate) fn finish(mut self, id: &str, final_answer: bool) -> Result<MessageBatch, String> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.process_line(&line)?;
        }
        if !self.ended {
            return self.fail("missing message terminator");
        }
        if self.current.is_some() {
            return self.fail("message is missing its closing tag");
        }
        for (index, message) in self.messages.iter_mut().enumerate() {
            message.id = format!("{id}:{}", index + 1);
        }
        Ok(MessageBatch {
            id: id.to_string(),
            sender: MessageRecipient::Supervisor,
            audience: MessageRecipient::User,
            messages: self.messages,
            final_answer,
        })
    }

    fn process_line(&mut self, line: &str) -> Result<(), String> {
        self.line_number += 1;
        if !self.started {
            if line != "<messages>" {
                return self.fail("expected <messages>");
            }
            self.started = true;
            return Ok(());
        }
        if self.ended {
            return self.fail("content after the message terminator");
        }
        if line == "</messages>" {
            if self.current.is_some() {
                return self.fail("close the current message before </messages>");
            }
            if self.messages.is_empty() {
                return self.fail("batch must contain at least one message");
            }
            self.ended = true;
            return Ok(());
        }
        if let Some(recipient) = self.current {
            let closing = match recipient {
                MessageRecipient::User => "</user>",
                MessageRecipient::Implementer => "</implementer>",
                MessageRecipient::Supervisor => unreachable!(),
            };
            if line == closing {
                let Some(message) = self.messages.last() else {
                    return self.fail("message body has no recipient block");
                };
                if message.text.trim().is_empty() {
                    return self.fail("message must contain nonempty text");
                }
                self.current = None;
                self.visible_markup_candidate.clear();
                return Ok(());
            }
            let Some(message) = self.messages.last_mut() else {
                return self.fail("message body has no recipient block");
            };
            message.text.push_str(line);
            message.text.push('\n');
            return Ok(());
        }
        let recipient = match line {
            "<user>" => MessageRecipient::User,
            "<implementer>" => MessageRecipient::Implementer,
            _ => return self.fail("expected <user>, <implementer>, or </messages>"),
        };
        if self.messages.len() == 8 {
            return self.fail("batch exceeds eight messages");
        }
        self.block_number = self.messages.len() + 1;
        self.messages.push(DirectedMessage {
            id: String::new(),
            recipient,
            text: String::new(),
        });
        self.current = Some(recipient);
        Ok(())
    }

    fn observe_stream_char(&mut self, ch: char) {
        if self.current != Some(MessageRecipient::User) {
            return;
        }
        let closing = "</user>";
        if self.visible_markup_candidate.is_empty() {
            if ch == '<' {
                self.visible_markup_candidate.push(ch);
            } else {
                self.visible_delta.push(ch);
            }
            return;
        }
        if ch == '\n' && self.visible_markup_candidate == closing {
            return;
        }
        self.visible_markup_candidate.push(ch);
        if closing.starts_with(&self.visible_markup_candidate) {
            return;
        }
        self.visible_delta.push_str(&self.visible_markup_candidate);
        self.visible_markup_candidate.clear();
    }

    fn fail<T>(&mut self, reason: &str) -> Result<T, String> {
        let error = format!(
            "Continuous Planning message error at line {}, block {}: {reason}",
            self.line_number.max(1),
            self.block_number.max(1)
        );
        self.error = Some(error.clone());
        Err(error)
    }
}

#[cfg(test)]
#[path = "message_parser_tests.rs"]
mod tests;
