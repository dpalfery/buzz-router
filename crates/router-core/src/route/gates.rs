//! The ordered gate helper of design section 5.5 (requirements 12.4, 19.2, 20.2 and 21.2).

use crate::config::Limits;
use crate::ids::BotName;
use crate::route::{Snapshot, SuppressWhy, WakeCounts};

/// Checks `bot` against `gates` in the order given and returns the first one that fails, or
/// `None` when every gate passes.
///
/// The gates are the `SuppressWhy` variants the helper can return:
///
/// - `Halted` fails when every bot is halted or this bot is halted.
/// - `Quiet` fails when the bot is in `snap.quiet`.
/// - `Cap` fails when the bot's turns used in the thread's current round have reached its
///   `turns_per_round`.
/// - `Budget` fails when the bot's hourly count has reached `wakes_per_hour` or its daily count
///   has reached `wakes_per_day`.
///
/// `RespondTo` is not a limit gate: the human and foreign rules decide it themselves from the
/// bot's `respond_to`, so it never fails here. A bot with no turn entry, no wake-count entry, or
/// no thread yet has used nothing. A bot missing from the roster is read against the default
/// limits.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called by the routing rules of tasks 1.7 to 1.9")
)]
pub(super) fn gate(
    bot: &BotName,
    gates: &[SuppressWhy],
    snap: &Snapshot<'_>,
) -> Option<SuppressWhy> {
    let limits = snap
        .roster
        .bots
        .get(bot)
        .map_or_else(Limits::default, |bot| bot.limits);
    gates.iter().copied().find(|why| match why {
        SuppressWhy::Halted => snap.halts.all || snap.halts.bots.contains(bot),
        SuppressWhy::Quiet => snap.quiet.contains(bot),
        SuppressWhy::Cap => {
            let turns_used = snap
                .thread
                .as_ref()
                .and_then(|thread| thread.turns_used.get(bot))
                .copied()
                .unwrap_or(0);
            turns_used >= limits.turns_per_round
        }
        SuppressWhy::Budget => {
            let counts = snap.wake_counts.get(bot).copied();
            let WakeCounts { hour, day } = counts.unwrap_or(WakeCounts { hour: 0, day: 0 });
            hour >= limits.wakes_per_hour || day >= limits.wakes_per_day
        }
        SuppressWhy::RespondTo => false,
    })
}

#[cfg(test)]
mod tests {
    //! Task 1.5 (RED): the ordered gate helper of design section 5.5.
    //!
    //! Design 5.5 fixes `gate(bot, gates, snap) -> Option<SuppressWhy>`: it evaluates `gates` in
    //! the order given and returns the first one that fails, or `None` when every gate passes.
    //! The gates are:
    //!
    //! - `Halted` when `halts.all || halts.bots.contains(bot)`;
    //! - `Quiet` when `quiet.contains(bot)`;
    //! - `Cap` when `thread.turns_used[bot] >= limits(bot).turns_per_round`;
    //! - `Budget` when `counts.hour >= wakes_per_hour || counts.day >= wakes_per_day`, with
    //!   `counts = snap.wake_counts[bot]`.
    //!
    //! Design 5.5 writes `gates` as the bare list `[Halted, Quiet, Cap, Budget]` and does not
    //! name its element type. The only names it can mean are the `SuppressWhy` variants that
    //! `gate` returns, so these tests call `gate(&BotName, &[SuppressWhy], &Snapshot<'_>)`.
    //!
    //! The roster is built through `parse_roster`: bot `A` overrides the limits with
    //! `turns_per_round = 2`, `wakes_per_hour = 3` and `wakes_per_day = 5`, so a bot read against
    //! the defaults (4, 20 and 100) would pass where `A` fails. Bot `B` keeps the defaults. Every
    //! key is a deterministic fixture key, `sha256("buzz-router-fixture:" + name)`.
    //!
    //! Beyond the contract's bullets (the three orderings, the hourly and daily counts, and "below
    //! every limit gives `None`"), the tests also cover what design 5.5 and acceptance criterion
    //! 2 state: each gate on its own, counts above a limit as well as at it, the `halts.all` arm,
    //! the list order being the evaluation order, a gate left out of the list not being evaluated
    //! (an owner edit gates `[Halted, Cap]` only, R16.3), and each bot being read against its own
    //! limits and its own entries. Missing map entries and a missing thread count as nothing used.

    use std::collections::{BTreeMap, BTreeSet};

    use nostr::{Keys, SecretKey};
    use sha2::{Digest, Sha256};

    use super::gate;
    use crate::config::{parse_roster, Limits, Roster};
    use crate::ids::{BotName, ChannelId, EventId};
    use crate::route::{Halts, Snapshot, SuppressWhy, WakeCounts};
    use crate::thread::{RoundMode, ThreadState};

    /// The limits that bot `A` overrides in the fixture roster.
    const A_TURNS: u32 = 2;
    const A_HOUR: u32 = 3;
    const A_DAY: u32 = 5;

    /// The gates in the order every bot-caused and human-caused target is checked (R12.4).
    const DESIGN_ORDER: [SuppressWhy; 4] = [
        SuppressWhy::Halted,
        SuppressWhy::Quiet,
        SuppressWhy::Cap,
        SuppressWhy::Budget,
    ];

    /// `sha256("buzz-router-fixture:" + name)`, the repository's fixture-key convention.
    fn fixture_digest(name: &str) -> [u8; 32] {
        Sha256::digest(format!("buzz-router-fixture:{name}")).into()
    }

    /// The 64-character lowercase hex public key of the fixture key `name`.
    fn pubkey_hex(name: &str) -> String {
        let secret = SecretKey::from_slice(&fixture_digest(name))
            .expect("a sha256 digest is a valid secret key");
        Keys::new(secret).public_key().to_hex()
    }

    fn bot_name(name: &str) -> BotName {
        BotName::new(name).expect("a fixture bot name is not blank")
    }

    /// The fixture roster: owner `O`, bot `A` with the overridden limits, bot `B` with the
    /// defaults. Both cover every channel.
    fn roster() -> Roster {
        let source = format!(
            r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["{owner}"]
timezone = "UTC"

[[bots]]
name = "A"
pubkey = "{bot_a}"
channels = ["*"]
respond_to = "owner-only"
limits = {{ turns_per_round = {A_TURNS}, wakes_per_hour = {A_HOUR}, wakes_per_day = {A_DAY} }}

[[bots]]
name = "B"
pubkey = "{bot_b}"
channels = ["*"]
respond_to = "owner-only"
"#,
            owner = pubkey_hex("O"),
            bot_a = pubkey_hex("A"),
            bot_b = pubkey_hex("B"),
        );
        parse_roster(&source).expect("the fixture roster is valid")
    }

    /// A thread in which `A` and `B` take part, with every bot's turn count set to 0.
    fn idle_thread() -> ThreadState {
        let channel_digest = fixture_digest("channel:room");
        let mut channel_bytes = [0_u8; 16];
        channel_bytes.copy_from_slice(&channel_digest[..16]);
        let event_id = |name: &str| {
            EventId::from_hex(&hex::encode(fixture_digest(&format!("event:{name}"))))
                .expect("a fixture event id is 64 hex characters")
        };
        ThreadState {
            root_id: event_id("root"),
            channel_id: ChannelId::from(uuid::Uuid::from_bytes(channel_bytes)),
            participants: [bot_name("A"), bot_name("B")].into_iter().collect(),
            discussion: true,
            round_id: event_id("root"),
            round_mode: RoundMode::Discussion,
            round_started_at: 0,
            turns_used: [(bot_name("A"), 0), (bot_name("B"), 0)]
                .into_iter()
                .collect(),
        }
    }

    /// Everything `gate` reads, owned in one place so a test can change one thing at a time.
    /// `idle()` starts with no halt, nobody quiet, a thread, and every counter at zero.
    struct Scenario {
        roster: Roster,
        local_bots: BTreeSet<BotName>,
        halts: Halts,
        quiet: BTreeSet<BotName>,
        thread: Option<ThreadState>,
        wake_counts: BTreeMap<BotName, WakeCounts>,
    }

    impl Scenario {
        fn idle() -> Self {
            let no_wakes = WakeCounts { hour: 0, day: 0 };
            Self {
                roster: roster(),
                local_bots: [bot_name("A"), bot_name("B")].into_iter().collect(),
                halts: Halts {
                    all: false,
                    bots: BTreeSet::new(),
                },
                quiet: BTreeSet::new(),
                thread: Some(idle_thread()),
                wake_counts: [(bot_name("A"), no_wakes), (bot_name("B"), no_wakes)]
                    .into_iter()
                    .collect(),
            }
        }

        /// `bot` is halted by name.
        fn halted(mut self, bot: &str) -> Self {
            self.halts.bots.insert(bot_name(bot));
            self
        }

        /// Every bot is halted.
        fn halted_everywhere(mut self) -> Self {
            self.halts.all = true;
            self
        }

        /// `bot` is inside quiet hours.
        fn quiet(mut self, bot: &str) -> Self {
            self.quiet.insert(bot_name(bot));
            self
        }

        /// `bot` has used `turns` turns in the thread's current round.
        fn turns_used(mut self, bot: &str, turns: u32) -> Self {
            if let Some(thread) = self.thread.as_mut() {
                thread.turns_used.insert(bot_name(bot), turns);
            }
            self
        }

        /// `bot` has dispatched `hour` wakes in the trailing hour and `day` in the trailing day.
        fn wakes(mut self, bot: &str, hour: u32, day: u32) -> Self {
            self.wake_counts
                .insert(bot_name(bot), WakeCounts { hour, day });
            self
        }

        /// Drops `bot`'s entries from the thread's turn counts and from the wake counts.
        fn without_entries_for(mut self, bot: &str) -> Self {
            let bot = bot_name(bot);
            if let Some(thread) = self.thread.as_mut() {
                thread.turns_used.remove(&bot);
            }
            self.wake_counts.remove(&bot);
            self
        }

        /// The event has no thread yet, as for a new top-level post.
        fn without_thread(mut self) -> Self {
            self.thread = None;
            self
        }

        /// What `gate` says about `bot` for `gates`.
        fn check(&self, bot: &str, gates: &[SuppressWhy]) -> Option<SuppressWhy> {
            let snapshot = Snapshot {
                roster: &self.roster,
                local_bots: &self.local_bots,
                local_members: self.local_bots.clone(),
                halts: &self.halts,
                thread: self.thread.clone(),
                parent_author: None,
                edit_target: None,
                wake_counts: &self.wake_counts,
                quiet: self.quiet.clone(),
            };
            gate(&bot_name(bot), gates, &snapshot)
        }
    }

    // -----------------------------------------------------------------------------------------
    // The fixture itself: the limits the other tests rely on
    // -----------------------------------------------------------------------------------------

    /// Guards the arithmetic of the tests below: `A` carries the overrides and `B` the defaults.
    #[test]
    fn the_fixture_roster_gives_a_the_overrides_and_b_the_defaults() {
        let roster = roster();

        assert_eq!(
            roster.bots["A"].limits,
            Limits {
                turns_per_round: A_TURNS,
                wakes_per_hour: A_HOUR,
                wakes_per_day: A_DAY,
                ..Limits::default()
            }
        );
        assert_eq!(roster.bots["B"].limits, Limits::default());
    }

    // -----------------------------------------------------------------------------------------
    // Each gate on its own
    // -----------------------------------------------------------------------------------------

    #[test]
    fn a_bot_halted_by_name_gives_halted() {
        let scenario = Scenario::idle().halted("A");

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
    }

    #[test]
    fn a_bot_halted_with_everyone_gives_halted() {
        let scenario = Scenario::idle().halted_everywhere();

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
        assert_eq!(
            scenario.check("B", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
    }

    #[test]
    fn a_quiet_bot_gives_quiet() {
        let scenario = Scenario::idle().quiet("A");

        assert_eq!(scenario.check("A", &DESIGN_ORDER), Some(SuppressWhy::Quiet));
    }

    #[test]
    fn turns_used_at_turns_per_round_give_cap() {
        let scenario = Scenario::idle().turns_used("A", A_TURNS);

        assert_eq!(scenario.check("A", &DESIGN_ORDER), Some(SuppressWhy::Cap));
    }

    /// Owner-caused wakes use turns too (R19.1), so a count can run past the limit.
    #[test]
    fn turns_used_above_turns_per_round_give_cap() {
        let scenario = Scenario::idle().turns_used("A", A_TURNS + 3);

        assert_eq!(scenario.check("A", &DESIGN_ORDER), Some(SuppressWhy::Cap));
    }

    // -----------------------------------------------------------------------------------------
    // The contract's budget bullet: the hourly and the daily count each give Budget
    // -----------------------------------------------------------------------------------------

    #[test]
    fn an_hourly_count_at_wakes_per_hour_gives_budget() {
        let scenario = Scenario::idle().wakes("A", A_HOUR, 0);

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Budget)
        );
    }

    /// Owner-caused wakes count toward the budget (R20.1), so a count can run past the limit.
    #[test]
    fn an_hourly_count_above_wakes_per_hour_gives_budget() {
        let scenario = Scenario::idle().wakes("A", A_HOUR + 4, 0);

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Budget)
        );
    }

    #[test]
    fn a_daily_count_at_wakes_per_day_gives_budget() {
        let scenario = Scenario::idle().wakes("A", 0, A_DAY);

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Budget)
        );
    }

    #[test]
    fn a_daily_count_above_wakes_per_day_gives_budget() {
        let scenario = Scenario::idle().wakes("A", 0, A_DAY + 4);

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Budget)
        );
    }

    // -----------------------------------------------------------------------------------------
    // The contract's ordering bullets: Halted over Quiet over Cap over Budget
    // -----------------------------------------------------------------------------------------

    #[test]
    fn halted_and_quiet_together_give_halted() {
        let scenario = Scenario::idle().halted("A").quiet("A");

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
    }

    #[test]
    fn halted_everywhere_and_quiet_together_give_halted() {
        let scenario = Scenario::idle().halted_everywhere().quiet("A");

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
    }

    #[test]
    fn quiet_and_over_cap_give_quiet() {
        let scenario = Scenario::idle().quiet("A").turns_used("A", A_TURNS);

        assert_eq!(scenario.check("A", &DESIGN_ORDER), Some(SuppressWhy::Quiet));
    }

    #[test]
    fn over_cap_and_over_budget_give_cap() {
        let scenario = Scenario::idle()
            .turns_used("A", A_TURNS)
            .wakes("A", A_HOUR, A_DAY);

        assert_eq!(scenario.check("A", &DESIGN_ORDER), Some(SuppressWhy::Cap));
    }

    #[test]
    fn every_gate_failing_at_once_gives_halted() {
        let scenario = Scenario::idle()
            .halted("A")
            .quiet("A")
            .turns_used("A", A_TURNS)
            .wakes("A", A_HOUR, A_DAY);

        assert_eq!(
            scenario.check("A", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
    }

    // -----------------------------------------------------------------------------------------
    // The contract's last bullet: below every limit gives None
    // -----------------------------------------------------------------------------------------

    /// One below each limit: turns, hourly wakes and daily wakes. Not halted, not quiet.
    #[test]
    fn below_every_limit_gives_none() {
        let scenario =
            Scenario::idle()
                .turns_used("A", A_TURNS - 1)
                .wakes("A", A_HOUR - 1, A_DAY - 1);

        assert_eq!(scenario.check("A", &DESIGN_ORDER), None);
    }

    /// A bot that has not been woken has no entry in either map. Both maps are sparse in the
    /// engine (conformance case 16 starts a bot-only thread with no turns recorded), so a missing
    /// entry is zero used.
    #[test]
    fn a_bot_with_no_recorded_turns_or_wakes_is_below_every_limit() {
        let scenario = Scenario::idle().without_entries_for("A");

        assert_eq!(scenario.check("A", &DESIGN_ORDER), None);
    }

    /// A new top-level post has no thread yet (`Snapshot.thread` is `None`), so nothing is used.
    #[test]
    fn a_bot_in_a_thread_that_does_not_exist_yet_is_below_every_limit() {
        let scenario = Scenario::idle().without_thread();

        assert_eq!(scenario.check("A", &DESIGN_ORDER), None);
    }

    // -----------------------------------------------------------------------------------------
    // Acceptance criterion 2: gates are evaluated in the order given
    // -----------------------------------------------------------------------------------------

    /// With every gate failing, the list `[Budget, Cap, Quiet, Halted]` returns `Budget`: the
    /// order of the list, not a fixed order inside `gate`, decides which gate wins.
    #[test]
    fn the_order_of_the_list_decides_which_gate_wins() {
        let scenario = Scenario::idle()
            .halted("A")
            .quiet("A")
            .turns_used("A", A_TURNS)
            .wakes("A", A_HOUR, A_DAY);
        let reversed = [
            SuppressWhy::Budget,
            SuppressWhy::Cap,
            SuppressWhy::Quiet,
            SuppressWhy::Halted,
        ];

        assert_eq!(scenario.check("A", &reversed), Some(SuppressWhy::Budget));
    }

    /// An owner edit gates `[Halted, Cap]` only (R16.3, A11). A bot that is quiet and over its
    /// budget, but neither halted nor capped, passes that list.
    #[test]
    fn a_gate_left_out_of_the_list_is_not_evaluated() {
        let scenario = Scenario::idle().quiet("A").wakes("A", A_HOUR, A_DAY);

        assert_eq!(
            scenario.check("A", &[SuppressWhy::Halted, SuppressWhy::Cap]),
            None
        );
    }

    // -----------------------------------------------------------------------------------------
    // Each bot is read against its own limits and its own entries
    // -----------------------------------------------------------------------------------------

    /// `B` has the same counts that stop `A`, but `B` has the default limits (R19.2, R20.2,
    /// R21.2 say "its effective limit").
    #[test]
    fn a_bot_with_the_same_counts_is_checked_against_its_own_limits() {
        let scenario = Scenario::idle()
            .turns_used("B", A_TURNS)
            .wakes("B", A_HOUR, A_DAY);

        assert_eq!(scenario.check("B", &DESIGN_ORDER), None);
    }

    /// `B` has no override, so the R2.5 defaults are its limits: each counter at the default
    /// limit gives its gate.
    #[test]
    fn a_bot_without_an_override_is_checked_against_the_defaults() {
        let defaults = Limits::default();

        let capped = Scenario::idle().turns_used("B", defaults.turns_per_round);
        assert_eq!(capped.check("B", &DESIGN_ORDER), Some(SuppressWhy::Cap));

        let hourly = Scenario::idle().wakes("B", defaults.wakes_per_hour, 0);
        assert_eq!(hourly.check("B", &DESIGN_ORDER), Some(SuppressWhy::Budget));

        let daily = Scenario::idle().wakes("B", 0, defaults.wakes_per_day);
        assert_eq!(daily.check("B", &DESIGN_ORDER), Some(SuppressWhy::Budget));
    }

    #[test]
    fn a_halt_on_another_bot_does_not_halt_this_one() {
        let scenario = Scenario::idle().halted("B");

        assert_eq!(scenario.check("A", &DESIGN_ORDER), None);
        assert_eq!(
            scenario.check("B", &DESIGN_ORDER),
            Some(SuppressWhy::Halted)
        );
    }

    #[test]
    fn another_bot_being_quiet_does_not_silence_this_one() {
        let scenario = Scenario::idle().quiet("B");

        assert_eq!(scenario.check("A", &DESIGN_ORDER), None);
        assert_eq!(scenario.check("B", &DESIGN_ORDER), Some(SuppressWhy::Quiet));
    }

    #[test]
    fn another_bots_counts_do_not_count_against_this_one() {
        let scenario = Scenario::idle().turns_used("B", 99).wakes("B", 99, 999);

        assert_eq!(scenario.check("A", &DESIGN_ORDER), None);
    }
}
