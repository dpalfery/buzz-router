//! Task 1.10 (RED): the offline replay simulator.
//!
//! Covers R55.2 to R55.4 (the simulator half) and assumption A17 against design section 5.8:
//! `Replayer::new(Roster)` treats every roster bot as local and as a member of every channel it
//! covers, and `Replayer::step(&mut self, &InEvent) -> RouteResult` then
//!
//! 1. derives `now` from the event's `created_at` (the simulated clock),
//! 2. builds a `Snapshot` from the simulator's own state: its thread map, an index of event id to
//!    author and root (for parent authors), its halts, and the wake counts and quiet set for that
//!    `now`,
//! 3. calls `route`, and
//! 4. applies the thread update and the control, and counts every `Wake` as dispatched at once.
//!
//! The simulator is the only source of state in these tests. A test never builds a `Snapshot`: it
//! feeds events in order and reads the decisions `step` returns, so "the thread update was
//! applied" shows up as the next event finding its thread, "a control was applied" as a later
//! `Suppress(Halted)` or a wake again, and "a wake counts as dispatched" as `Suppress(Cap)`.
//!
//! Each test maps to one line of the task's Behaviour list:
//!
//! - an owner `@everyone`, then bot posts, gives Cap after 4 wakes for each bot:
//!   `owner_everyone_then_bot_posts_give_cap_after_four_wakes_for_each_bot`;
//! - `stop`, then `@A hi`, gives `Suppress(Halted)`; `resume`, then `@A hi`, gives a wake:
//!   `stop_suppresses_a_mention_as_halted_and_resume_wakes_again`;
//! - a bot post whose `created_at` falls inside quiet hours gives Quiet:
//!   `bot_post_inside_quiet_hours_gives_quiet`;
//! - replies resolve their parent author from earlier events:
//!   `a_reply_resolves_its_parent_author_from_an_earlier_event`.
//!
//! One test goes beyond the four lines, for acceptance criterion 1 ("the simulated clock is
//! `now = created_at`"): `quiet_hours_follow_each_events_created_at` runs one replayer across
//! events outside, inside and outside quiet hours, so a clock fixed at the first event, or at
//! anything but each event's own `created_at`, fails it.
//!
//! The roster is `common::roster()`: owner `O`, bots `A`, `B` and `C` (all owner-only, all
//! covering every channel), no `default_bot`, UTC, and the R2.5 default limits (`turns_per_round`
//! 4, 20 wakes an hour, quiet hours `23:00-07:00`). Every event is a kind-9 post in the fixture
//! channel `room`, and every key is a deterministic fixture key.

mod common;

use common::{bot_name, event_id, event_id_hex, keys, message, reply_tags, roster};
use router_core::replay::Replayer;
use router_core::route::{Control, Decision, InEvent, Priority, Reason, Scope, SuppressWhy};

/// 2023-11-14T00:00:00Z in unix seconds. Every event time below is an offset from it, so each is
/// a wall-clock time of day in the fixture roster's owner timezone, UTC.
const MIDNIGHT: i64 = 1_699_920_000;

/// The unix time of `hour:minute` on 2023-11-14 UTC. An `hour` of 24 or more reaches the next
/// day: `at(32, 0)` is 08:00 on 2023-11-15.
fn at(hour: i64, minute: i64) -> i64 {
    MIDNIGHT + hour * 3600 + minute * 60
}

/// A top-level kind-9 post by the fixture key `author`. `label` names the event: it becomes the
/// event id through `common::event_id`, so two events of one replay need two labels.
fn top_level(label: &str, author: &str, content: &str, created_at: i64) -> InEvent {
    InEvent {
        id: event_id(label),
        created_at,
        ..message(&keys(author), content, Vec::new())
    }
}

/// A kind-9 reply by `author` in the thread rooted at the event labelled `root`, to the event
/// labelled `parent`. When `parent == root` it is a direct reply to the root. The `e` tags are the
/// ones `buzz_sdk` writes (`common::reply_tags`).
fn reply(
    label: &str,
    author: &str,
    content: &str,
    created_at: i64,
    root: &str,
    parent: &str,
) -> InEvent {
    InEvent {
        id: event_id(label),
        created_at,
        ..message(
            &keys(author),
            content,
            reply_tags(&event_id_hex(root), &event_id_hex(parent)),
        )
    }
}

/// `Decision::Wake` for the bot named `bot`.
fn wake(bot: &str, reason: Reason, priority: Priority, debounce: bool) -> Decision {
    Decision::Wake {
        bot: bot_name(bot),
        reason,
        priority,
        debounce,
    }
}

/// `Decision::Suppress` for the bot named `bot`.
fn suppress(bot: &str, why: SuppressWhy) -> Decision {
    Decision::Suppress {
        bot: bot_name(bot),
        why,
    }
}

/// How many of `decisions` are a `Wake` for the bot named `bot`.
fn wakes_for(decisions: &[Decision], bot: &str) -> usize {
    decisions
        .iter()
        .filter(|decision| matches!(decision, Decision::Wake { bot: woken, .. } if woken.as_str() == bot))
        .count()
}

/// Behaviour 1. The owner's top-level `@everyone` wakes every roster bot (all of them are local),
/// starts a Discussion round and makes all three participants. Each wake counts as a turn at once
/// (R6.7, R19.1), so the owner's wake is turn 1 and the bots' posts then wake each other until
/// every bot has had 4 wakes (R19.6: one round costs at most 4 x the number of bots). The next
/// attempt to wake a bot that has had its 4 is `Suppress(Cap)`, and a capped bot stays capped.
///
/// The posts rotate A, B, C, A, B, C, A as direct replies to the owner's post. The turns used
/// after each post, as `A/B/C`: owner 1/1/1, A posts 1/2/2, B posts 2/2/3, C posts 3/3/3, A posts
/// 3/4/4, B posts 4/4/4 (A woken, C capped), C posts (A and B capped), A posts (B and C capped).
#[test]
fn owner_everyone_then_bot_posts_give_cap_after_four_wakes_for_each_bot() {
    let mut replayer = Replayer::new(roster());
    let mut seen: Vec<Decision> = Vec::new();

    let everyone = replayer.step(&top_level("root", "O", "@everyone thoughts?", at(12, 0)));
    assert_eq!(
        everyone.decisions,
        vec![
            wake("A", Reason::Everyone, Priority::Owner, false),
            wake("B", Reason::Everyone, Priority::Owner, false),
            wake("C", Reason::Everyone, Priority::Owner, false),
        ],
        "the owner's @everyone wakes every roster bot"
    );
    seen.extend(everyone.decisions);

    let discussion = |bot: &str| wake(bot, Reason::Discussion, Priority::Bot, true);
    let capped = |bot: &str| suppress(bot, SuppressWhy::Cap);
    let posts = [
        ("A", vec![discussion("B"), discussion("C")]),
        ("B", vec![discussion("A"), discussion("C")]),
        ("C", vec![discussion("A"), discussion("B")]),
        ("A", vec![discussion("B"), discussion("C")]),
        ("B", vec![discussion("A"), capped("C")]),
        ("C", vec![capped("A"), capped("B")]),
        ("A", vec![capped("B"), capped("C")]),
    ];
    for (minute, (author, expected)) in (1_i64..).zip(posts) {
        let post = reply(
            &format!("post-{minute}"),
            author,
            "my take",
            at(12, minute),
            "root",
            "root",
        );
        let result = replayer.step(&post);
        assert_eq!(
            result.decisions, expected,
            "decisions for post {minute}, by {author}"
        );
        seen.extend(result.decisions);
    }

    for bot in ["A", "B", "C"] {
        assert_eq!(
            wakes_for(&seen, bot),
            4,
            "{bot} is woken exactly turns_per_round times in the round"
        );
    }
}

/// Behaviour 2. `stop` halts every bot, so the owner's `@A hi` is `Suppress(Halted)` (the only
/// gate on an owner's wake, R6.8). `resume` lifts the halt, so the next `@A hi` is a wake again.
#[test]
fn stop_suppresses_a_mention_as_halted_and_resume_wakes_again() {
    let mut replayer = Replayer::new(roster());

    let stop = replayer.step(&top_level("stop", "O", "stop", at(12, 0)));
    assert_eq!(stop.control, Some(Control::Stop(Scope::All)));

    let halted = replayer.step(&top_level("halted-hi", "O", "@A hi", at(12, 1)));
    assert_eq!(halted.decisions, vec![suppress("A", SuppressWhy::Halted)]);

    let resume = replayer.step(&top_level("resume", "O", "resume", at(12, 2)));
    assert_eq!(resume.control, Some(Control::Resume(Scope::All)));

    let woken = replayer.step(&top_level("woken-hi", "O", "@A hi", at(12, 3)));
    assert_eq!(
        woken.decisions,
        vec![wake("A", Reason::Mention, Priority::Owner, false)]
    );
}

/// Behaviour 3. Bot `A`'s top-level post `@B hi` at 23:30 UTC is inside the default quiet hours
/// `23:00-07:00`, so `B`, which it wakes by mention, is suppressed `Quiet` (a bot-caused wake is
/// gated Halted, Quiet, Cap, Budget, R12.4). The clock is the post's own `created_at`.
#[test]
fn bot_post_inside_quiet_hours_gives_quiet() {
    let mut replayer = Replayer::new(roster());

    let result = replayer.step(&top_level("quiet-post", "A", "@B hi", at(23, 30)));

    assert_eq!(result.decisions, vec![suppress("B", SuppressWhy::Quiet)]);
}

/// Acceptance criterion 1, beyond the Behaviour list. One replayer sees the same bot post at
/// 12:00, at 23:30 and at 08:00 the next day: the wake is `Quiet` only for the middle one, so the
/// quiet set follows each event's own `created_at`, in both directions, and does not latch.
#[test]
fn quiet_hours_follow_each_events_created_at() {
    let mut replayer = Replayer::new(roster());
    let bot_wake = || vec![wake("B", Reason::BotMention, Priority::Bot, true)];

    let midday = replayer.step(&top_level("midday", "A", "@B hi", at(12, 0)));
    assert_eq!(midday.decisions, bot_wake(), "12:00 is outside quiet hours");

    let night = replayer.step(&top_level("night", "A", "@B hi", at(23, 30)));
    assert_eq!(
        night.decisions,
        vec![suppress("B", SuppressWhy::Quiet)],
        "23:30 is inside quiet hours"
    );

    let morning = replayer.step(&top_level("morning", "A", "@B hi", at(32, 0)));
    assert_eq!(
        morning.decisions,
        bot_wake(),
        "08:00 the next day is outside quiet hours"
    );
}

/// Behaviour 4. The owner starts a thread with `@A @B go`; `A` and `B` each reply to the root. An
/// untagged owner reply to `B`'s reply wakes `B` alone, as `ReplyTarget` (rule (c), R8.1): the
/// simulator resolved the parent's author from `B`'s earlier event. Without that, the reply would
/// fall to rule (d) and wake both participants as `Participant`. The same holds for an untagged
/// owner reply to `A`'s reply, which wakes `A` alone.
#[test]
fn a_reply_resolves_its_parent_author_from_an_earlier_event() {
    let mut replayer = Replayer::new(roster());

    let start = replayer.step(&top_level("root", "O", "@A @B go", at(12, 0)));
    assert_eq!(
        start.decisions,
        vec![
            wake("A", Reason::Mention, Priority::Owner, false),
            wake("B", Reason::Mention, Priority::Owner, false),
        ]
    );
    let a_reply = replayer.step(&reply("a-reply", "A", "done", at(12, 1), "root", "root"));
    assert!(a_reply.decisions.is_empty(), "a Direct round wakes nobody");
    let b_reply = replayer.step(&reply("b-reply", "B", "done", at(12, 2), "root", "root"));
    assert!(b_reply.decisions.is_empty(), "a Direct round wakes nobody");

    let to_b = replayer.step(&reply("to-b", "O", "thanks", at(12, 3), "root", "b-reply"));
    assert_eq!(
        to_b.decisions,
        vec![wake("B", Reason::ReplyTarget, Priority::Owner, false)]
    );

    let to_a = replayer.step(&reply("to-a", "O", "and you", at(12, 4), "root", "a-reply"));
    assert_eq!(
        to_a.decisions,
        vec![wake("A", Reason::ReplyTarget, Priority::Owner, false)]
    );
}
