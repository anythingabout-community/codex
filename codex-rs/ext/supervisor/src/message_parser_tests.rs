use super::*;
use pretty_assertions::assert_eq;

#[test]
fn arbitrary_stream_boundaries_preserve_messages_and_literal_markers() {
    let input = "*** Begin Messages\r\n*** Message To: User\r\n+核实中\r\n*** Message To: Implementer\r\n+  preserve indentation\r\n+\r\n+*** End Messages\r\n++literal plus\r\n*** Message To: User\r\n+继续\r\n*** End Messages";
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
                text: "  preserve indentation\n\n*** End Messages\n+literal plus\n".into(),
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
        "*** Message To: Supervisor\n+spoof\n*** End Messages",
        "*** Message To: User\n+ \n*** End Messages",
        "*** End Messages\ntrailing",
        "",
    ] {
        let mut parser = MessageParser::default();
        let result = parser
            .push(&format!(
                "*** Begin Messages\n*** Message To: Implementer\n+execute\n{tail}"
            ))
            .and_then(|()| parser.finish("source", /*final_answer*/ false));
        let error = result.unwrap_err();
        assert!(error.contains("line") && error.contains("block"), "{error}");
    }
}

#[test]
fn bounds_are_enforced_before_finish() {
    let mut parser = MessageParser::default();
    parser
        .push("*** Begin Messages\n*** Message To: User\n+")
        .unwrap();
    assert!(parser.push(&"x".repeat(MAX_BATCH_BYTES)).is_err());
    assert!(parser.finish("source", /*final_answer*/ false).is_err());
    let mut parser = MessageParser::default();
    parser.push("*** Begin Messages\n").unwrap();
    for _ in 0..8 {
        parser.push("*** Message To: User\n+text\n").unwrap();
    }
    assert!(parser.push("*** Message To: Implementer\n").is_err());
}
