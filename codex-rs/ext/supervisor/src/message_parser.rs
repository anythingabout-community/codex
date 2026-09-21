//! A bounded, side-effect-free protocol, modeled on Apply Patch's block grammar.
//!
//! batch := "*** Begin Messages" LF message{1,8} "*** End Messages" LF?
//! message := "*** Message To: " ("User" | "Implementer") LF ("+" text LF)+
//! Only a successfully finished parser can produce a dispatchable batch.

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
    messages: Vec<DirectedMessage>,
    error: Option<String>,
}

impl codex_extension_api::MessageStreamValidator for MessageParser {
    fn push(&mut self, delta: &str) -> Result<(), String> {
        MessageParser::push(self, delta)
    }
}

impl MessageParser {
    pub(crate) fn push(&mut self, delta: &str) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        for ch in delta.chars() {
            if ch.len_utf8() > MAX_BATCH_BYTES.saturating_sub(self.bytes) {
                self.line_number += 1;
                return self.fail("batch exceeds 8 KiB");
            }
            self.bytes += ch.len_utf8();
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
            return self.fail("missing *** End Messages");
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
            if line != "*** Begin Messages" {
                return self.fail("expected *** Begin Messages");
            }
            self.started = true;
            return Ok(());
        }
        if self.ended {
            return self.fail("content after *** End Messages");
        }
        if let Some(text) = line.strip_prefix('+') {
            let Some(message) = self.messages.last_mut() else {
                return self.fail("body before recipient");
            };
            message.text.push_str(text);
            message.text.push('\n');
            return Ok(());
        }
        if self
            .messages
            .last()
            .is_some_and(|message| message.text.trim().is_empty())
        {
            return self.fail("message must contain nonempty text");
        }
        if line == "*** End Messages" {
            if self.messages.is_empty() {
                return self.fail("batch must contain at least one message");
            }
            self.ended = true;
            return Ok(());
        }
        if line.starts_with("*** Message To:") {
            self.block_number = self.messages.len() + 1;
        }
        let recipient =
            match line {
                "*** Message To: User" => MessageRecipient::User,
                "*** Message To: Implementer" => MessageRecipient::Implementer,
                _ => return self.fail(
                    "expected a User or Implementer block, a '+' body line, or *** End Messages",
                ),
            };
        if self.messages.len() == 8 {
            return self.fail("batch exceeds eight messages");
        }
        self.messages.push(DirectedMessage {
            id: String::new(),
            recipient,
            text: String::new(),
        });
        Ok(())
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
