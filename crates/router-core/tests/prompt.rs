//! Prompt rendering: the built-in template, `render`, `reason_text` and `render_context`
//! (task 3.1; requirements 41.1–41.4, design section 5.7, DD-17).

use router_core::payload::ContextMessage;
use router_core::prompt::{reason_text, render, render_context, PromptVars, BUILT_IN_TEMPLATE};
use router_core::route::Reason;

const EXPECTED_TEMPLATE: &str = "You are {bot} in the Buzz channel #{channel}.
You were woken because: {reason_text}.
This is turn {turn} of {turns_per_round} for you in this thread until David posts again.

Thread so far (oldest first, ★ = new since your last turn):
{context}

Write the single message you want to post in this thread.
If you have nothing useful to add, output exactly: [no-reply]
Do not post acknowledgements (\"got it\", \"on it\", \"agreed\", \"standing by\").
David already sees a 👀 reaction when you are woken.
";

fn vars() -> PromptVars {
    PromptVars {
        bot: "A".to_owned(),
        channel: "work".to_owned(),
        reason_text: "David mentioned you".to_owned(),
        turn: 2,
        turns_per_round: 4,
        context: "★ David: hi".to_owned(),
    }
}

fn message(author: &str, text: &str, new: bool) -> ContextMessage {
    ContextMessage {
        id: "0".repeat(64),
        author: author.to_owned(),
        class: "owner".to_owned(),
        created_at: "2026-10-05T03:00:00Z".to_owned(),
        text: text.to_owned(),
        new,
    }
}

#[test]
fn built_in_template_is_the_brief_text() {
    assert_eq!(BUILT_IN_TEMPLATE, EXPECTED_TEMPLATE);
}

#[test]
fn render_fills_the_built_in_template() {
    let expected = "You are A in the Buzz channel #work.
You were woken because: David mentioned you.
This is turn 2 of 4 for you in this thread until David posts again.

Thread so far (oldest first, ★ = new since your last turn):
★ David: hi

Write the single message you want to post in this thread.
If you have nothing useful to add, output exactly: [no-reply]
Do not post acknowledgements (\"got it\", \"on it\", \"agreed\", \"standing by\").
David already sees a 👀 reaction when you are woken.
";
    assert_eq!(render(BUILT_IN_TEMPLATE, &vars()), expected);
}

#[test]
fn render_replaces_exactly_the_six_variables() {
    let template = "{bot}|{channel}|{reason_text}|{turn}|{turns_per_round}|{context}|{other}|{bot";
    assert_eq!(
        render(template, &vars()),
        "A|work|David mentioned you|2|4|★ David: hi|{other}|{bot"
    );
}

#[test]
fn render_does_not_expand_variables_inside_values() {
    let mut vars = vars();
    vars.context = "{bot} said {turn}".to_owned();
    assert_eq!(render("{context}", &vars), "{bot} said {turn}");
}

#[test]
fn reason_text_covers_all_seven_reasons() {
    let cases = [
        (Reason::Mention, "David mentioned you"),
        (
            Reason::Everyone,
            "David asked everyone; this is a discussion",
        ),
        (Reason::ReplyTarget, "David replied to your message"),
        (
            Reason::Participant,
            "David replied in a thread you're part of",
        ),
        (
            Reason::Discussion,
            "another bot posted in a discussion you're part of",
        ),
        (Reason::BotMention, "B mentioned you"),
        (
            Reason::DefaultBot,
            "you're the default bot for this channel",
        ),
    ];
    for (reason, expected) in cases {
        assert_eq!(reason_text(reason, "B"), expected, "{reason:?}");
    }
}

#[test]
fn render_context_marks_new_messages_and_indents_continuations() {
    let messages = [
        message("David", "first", false),
        message("B", "line one\nline two\nline three", true),
    ];
    assert_eq!(
        render_context(&messages),
        "  David: first\n★ B: line one\n    line two\n    line three"
    );
}

#[test]
fn render_context_of_no_messages_is_empty() {
    assert_eq!(render_context(&[]), "");
}
