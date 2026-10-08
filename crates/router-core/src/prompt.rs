//! Prompt rendering (design section 5.7, requirement 41, DD-17).

use crate::payload::ContextMessage;
use crate::route::Reason;

/// The built-in prompt template, byte-identical to brief section 9.4 (requirement 41.1).
pub const BUILT_IN_TEMPLATE: &str = "You are {bot} in the Buzz channel #{channel}.
You were woken because: {reason_text}.
This is turn {turn} of {turns_per_round} for you in this thread until David posts again.

Thread so far (oldest first, ★ = new since your last turn):
{context}

Write the single message you want to post in this thread.
If you have nothing useful to add, output exactly: [no-reply]
Do not post acknowledgements (\"got it\", \"on it\", \"agreed\", \"standing by\").
David already sees a 👀 reaction when you are woken.
";

/// The values of the six template variables (requirement 41.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptVars {
    /// `{bot}`: the bot's name.
    pub bot: String,
    /// `{channel}`: the channel name.
    pub channel: String,
    /// `{reason_text}`: from [`reason_text`].
    pub reason_text: String,
    /// `{turn}`: this wake's 1-based turn number in the round.
    pub turn: u32,
    /// `{turns_per_round}`: the bot's effective limit.
    pub turns_per_round: u32,
    /// `{context}`: from [`render_context`].
    pub context: String,
}

/// Replaces `{bot}`, `{channel}`, `{reason_text}`, `{turn}`, `{turns_per_round}` and `{context}`
/// in one pass, so text inside a substituted value is never expanded. Any other `{…}` text is
/// left as is (requirement 41.2).
pub fn render(template: &str, vars: &PromptVars) -> String {
    let turn = vars.turn.to_string();
    let turns_per_round = vars.turns_per_round.to_string();
    let substitutions: [(&str, &str); 6] = [
        ("{bot}", &vars.bot),
        ("{channel}", &vars.channel),
        ("{reason_text}", &vars.reason_text),
        ("{turn}", &turn),
        ("{turns_per_round}", &turns_per_round),
        ("{context}", &vars.context),
    ];

    let mut out = String::with_capacity(template.len() + vars.context.len());
    let mut rest = template;
    while let Some(brace) = rest.find('{') {
        out.push_str(&rest[..brace]);
        let tail = &rest[brace..];
        match substitutions
            .iter()
            .find(|(name, _)| tail.starts_with(name))
        {
            Some((name, value)) => {
                out.push_str(value);
                rest = &tail[name.len()..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `{reason_text}` sentence for `reason` (requirement 41.4). `author_name` fills the
/// `BotMention` sentence and is ignored otherwise.
pub fn reason_text(reason: Reason, author_name: &str) -> String {
    match reason {
        Reason::Mention => "David mentioned you".to_owned(),
        Reason::Everyone => "David asked everyone; this is a discussion".to_owned(),
        Reason::ReplyTarget => "David replied to your message".to_owned(),
        Reason::Participant => "David replied in a thread you're part of".to_owned(),
        Reason::Discussion => "another bot posted in a discussion you're part of".to_owned(),
        Reason::BotMention => format!("{author_name} mentioned you"),
        Reason::DefaultBot => "you're the default bot for this channel".to_owned(),
    }
}

/// The `{context}` text: one line per message, oldest first, `★ ` before new messages and two
/// spaces before the rest, with continuation lines indented 4 spaces (DD-17).
pub fn render_context(messages: &[ContextMessage]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let marker = if message.new { "★ " } else { "  " };
        let mut text = message.text.lines();
        let first = text.next().unwrap_or_default();
        lines.push(format!("{marker}{}: {first}", message.author));
        lines.extend(text.map(|line| format!("    {line}")));
    }
    lines.join("\n")
}
