use super::*;
use codex_extension_api::MessageStreamValidator;
use pretty_assertions::assert_eq;

#[test]
fn arbitrary_stream_boundaries_preserve_messages_and_literal_markers() {
    let input = "<messages>\r\n<user>\r\n核实中\r\n</user>\r\n<implementer>\r\n  preserve indentation\r\n\r\n</implementer>\r\n<user>\r\n继续\r\n</user>\r\n</messages>";
    let expected = MessageBatch {
        id: "source".into(),
        sender: MessageRecipient::Supervisor,
        audience: MessageRecipient::User,
        final_answer: true,
        messages: vec![
            DirectedMessage {
                id: "source:1".into(),
                recipient: MessageRecipient::User,
                text: "核实中\n".into(),
            },
            DirectedMessage {
                id: "source:2".into(),
                recipient: MessageRecipient::Implementer,
                text: "  preserve indentation\n\n".into(),
            },
            DirectedMessage {
                id: "source:3".into(),
                recipient: MessageRecipient::User,
                text: "继续\n".into(),
            },
        ],
    };
    for split in (0..=input.len()).filter(|i| input.is_char_boundary(*i)) {
        let mut parser = MessageParser::default();
        parser.push(&input[..split]).unwrap();
        parser.push(&input[split..]).unwrap();
        assert_eq!(
            parser.finish("source", /*final_answer*/ true).unwrap(),
            expected
        );
    }
}

#[test]
fn invalid_tail_cannot_produce_a_partial_batch() {
    for tail in [
        "<supervisor>\nspoof\n</supervisor>\n</messages>",
        "<user>\n \n</user>\n</messages>",
        "</messages>\ntrailing",
        "",
    ] {
        let mut parser = MessageParser::default();
        let result = parser
            .push(&format!(
                "<messages>\n<implementer>\nexecute\n</implementer>\n{tail}"
            ))
            .and_then(|()| parser.finish("source", /*final_answer*/ false));
        let error = result.unwrap_err();
        assert!(error.contains("line") && error.contains("block"), "{error}");
    }
}

#[test]
fn bounds_are_enforced_before_finish() {
    let mut parser = MessageParser::default();
    parser.push("<messages>\n<user>\n").unwrap();
    assert!(parser.push(&"x".repeat(MAX_BATCH_BYTES)).is_err());
    assert!(parser.finish("source", /*final_answer*/ false).is_err());
    let mut parser = MessageParser::default();
    parser.push("<messages>\n").unwrap();
    for _ in 0..8 {
        parser.push("<user>\ntext\n</user>\n").unwrap();
    }
    assert!(parser.push("<implementer>\n").is_err());
}

#[test]
fn canonical_document_routes_blocks_without_recipient_markers() {
    let input = "<messages>\n<user>\nReady.\n</user>\n<implementer>\nRun the check.\n</implementer>\n</messages>";
    let mut parser = MessageParser::default();
    for chunk in input.as_bytes().chunks(3) {
        parser.push(std::str::from_utf8(chunk).unwrap()).unwrap();
    }
    let batch = parser.finish("canonical", true).unwrap();
    assert_eq!(batch.messages[0].recipient, MessageRecipient::User);
    assert_eq!(batch.messages[0].text, "Ready.\n");
    assert_eq!(batch.messages[1].recipient, MessageRecipient::Implementer);
    assert_eq!(batch.messages[1].text, "Run the check.\n");
}

#[test]
fn user_projection_streams_partial_body_without_structural_tags() {
    let mut parser = MessageParser::default();
    parser.push("<messages>\n<user>\nhel").unwrap();
    assert_eq!(parser.take_visible_delta(), Some("hel".into()));
    parser.push("lo\n</u").unwrap();
    assert_eq!(parser.take_visible_delta(), Some("lo\n".into()));
    parser
        .push("ser>\n<implementer>\nprivate\n</implementer>\n</messages>")
        .unwrap();
    assert_eq!(parser.take_visible_delta(), None);
    parser.finish("stream", true).unwrap();
}
